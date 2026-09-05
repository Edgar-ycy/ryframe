"""为复制核对读取当前精确目标对象；仅明确 HEAD 404 可以形成缺失证据。"""
from __future__ import annotations

import copy
from pathlib import Path
import re
import subprocess

from devex_clone_capture import (CaptureReader, CaptureStages, inspect_response, read_json, regular_file, request_data,
                                validate_request, verify_capture, write_json)
from devex_clone_model import digest, linked, local_path
from devex_clone_object_batch import ordered_batches
from devex_clone_transfer import ObjectObservation
from restore_build import file_digest
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash


class ObjectObservationError(RuntimeError):
    def __init__(self, directory: Path):
        self.evidence_directory = str(directory)
        super().__init__(f"当前目标对象观察未通过；证据保留于 {directory}")


def missing_head(diagnostic: dict) -> bool:
    """只接受 AWS CLI 服务响应失败，不能用超时、403、GET 或泛化错误推断缺失。"""
    if (set(diagnostic) != {"operation", "returncode", "error_type", "stderr"}
            or diagnostic["operation"] != "head-object" or type(diagnostic["returncode"]) is not int
            or diagnostic["returncode"] != 254 or diagnostic["error_type"] != "CalledProcessError"
            or not isinstance(diagnostic["stderr"], str)):
        return False
    pattern = (r"(?:aws: \[ERROR\]: )?An error occurred \((?:404|NotFound)\) when calling the HeadObject operation"
               r"(?: \(reached max retries: 0\))?: Not Found")
    return re.fullmatch(pattern, diagnostic["stderr"].strip("\r\n")) is not None


def current_binding(tools: ExternalTools) -> dict:
    tools.command("aws")
    _, environment = tools.aws_context("target")
    return {"plan_sha256": plan_hash(tools.plan), "environment_sha256": plan_hash(environment)}


def fixed_tools(tools: ExternalTools, output: Path) -> tuple[dict, ExternalTools]:
    initial = current_binding(tools)
    _, environment = tools.aws_context("target")
    if plan_hash(environment) != initial["environment_sha256"]:
        raise ValueError("对象观察初始认证环境发生变化")

    def fixed_read(command, **kwargs):
        if current_binding(tools) != initial or kwargs.get("env") != environment:
            raise ValueError("对象请求前的计划或认证环境发生变化")
        kwargs["env"] = dict(environment)
        try:
            return tools.run(command, **kwargs)
        finally:
            if current_binding(tools) != initial:
                raise ValueError("对象请求期间的计划或认证环境发生变化")

    return initial, ExternalTools(copy.deepcopy(tools.plan), output, fixed_read)


def read_observation(output: Path, *, expected_sha256: str) -> dict:
    """按调用方已绑定摘要核验保存证据；拒绝失败残留，不证明此刻对象状态。"""
    digest(expected_sha256)
    marker = output / "failure.json"
    if marker.exists() or linked(marker):
        raise ValueError("对象观察失败，不能消费残留收据")
    receipt_path = regular_file(output / "observation.json")
    if file_digest(receipt_path)["sha256"] != expected_sha256:
        raise ValueError("对象观察收据与外部已绑定摘要不符")
    receipt = read_json(receipt_path)
    for name, expected in receipt["evidence"].items():
        if Path(name).name != name or file_digest(regular_file(output / name)) != expected:
            raise ValueError("对象观察原始证据变化或越界")
    if receipt["capture"] is not None and file_digest(regular_file(output / "capture/capture.json")) != receipt["capture"]:
        raise ValueError("对象完整采集收据变化")
    if file_digest(regular_file(receipt_path))["sha256"] != expected_sha256:
        raise ValueError("对象观察复核期间收据变化")
    if marker.exists() or linked(marker):
        raise ValueError("对象观察复核期间出现失败标记")
    return receipt


def probe_head(reader: CaptureReader, phase: str, bucket: str, key: str) -> bool:
    try:
        reader.read(phase, "head-object", bucket, key)
        return True
    except subprocess.CalledProcessError:
        if not missing_head(read_json(reader.output / f"{phase}.diagnostic.json")):
            raise
        return False


def observe_target_object(backend: Path, tools: ExternalTools, bucket: str, key: str, output: Path,
                          *, expected: dict, max_bytes: int) -> ObjectObservation:
    """实际观察当前目标；不能证明生产者停止、初始化历史或整个复制成功。

    存在时复用完整 HEAD/条件 GET/HEAD 采集；不存在时核验两次 HEAD 404 和所属桶 owner。
    发生未知结果时保存证据并退出，不降级、不重试或写对象。
    消费保存的结果必须调用 read_observation，失败目录的残留文件不能作为成功证据。
    """
    output = local_path(backend.resolve(), str(output), new=True)
    local_path(backend.resolve(), str(tools.work))
    validate_request(tools, "target", bucket, key, output, expected, max_bytes)
    output.mkdir()
    initial = None
    stage = "binding"
    try:
        initial, frozen = fixed_tools(tools, output)
        request = request_data(frozen, "target", bucket, key, expected, max_bytes)
        write_json(output / "intent.json", {**request, "kind": "devex-clone-live-object", "binding": initial})
        resource = {"kind": "object", "scope_id": request["scope_id"], "endpoint": request["endpoint"],
                    "bucket": bucket, "key": key}
        reader = CaptureReader(frozen, "target", output)
        capture = CaptureStages(reader, bucket, key, expected, max_bytes, owner_buckets=(bucket,))
        stage = "owners_before"
        owner_before = capture.owners_before()
        stage = "head_before"
        try:
            capture.head_before()
            exists = True
        except subprocess.CalledProcessError:
            if not missing_head(read_json(output / "head-before.diagnostic.json")):
                raise
            exists = False
        directory = None
        if exists:
            stage = "capture"
            directory = output / "capture"
            captured = capture.complete(directory)
            verify_capture(frozen, "target", bucket, key, directory, expected=expected, max_bytes=max_bytes, owner_buckets=(bucket,))
            owner_after = captured["ownership_after"]
        else:
            stage = "owners_after"
            owner_after = reader.owners("owner-after", (bucket,))
            stage = "head_after"
            if probe_head(reader, "head-after", bucket, key):
                raise ValueError("目标对象在两次缺失观察之间出现，不能报告缺失")
        stage = "binding_after"
        if current_binding(tools) != initial or current_binding(frozen) != initial:
            raise ValueError("对象观察期间目标计划、认证环境或工具发生变化")
        evidence = {path.name: file_digest(path) for path in sorted(output.iterdir()) if path.is_file()}
        receipt = {"format_version": 1, "status": "object_present" if exists else "object_absent",
                   "resource": resource, "binding": initial, "evidence": evidence,
                   "ownership_before": owner_before, "ownership_after": owner_after,
                   "capture": None if directory is None else file_digest(directory / "capture.json"),
                   "remote_writes": 0, "clone_verified": False, "restore_qualified": False}
        stage = "publish"
        if any(file_digest(output / name) != binding for name, binding in evidence.items()):
            raise ValueError("发布对象观察前原始证据变化")
        write_json(output / "observation.json", receipt)
        receipt_sha256 = file_digest(output / "observation.json")["sha256"]
        if read_observation(output, expected_sha256=receipt_sha256) != receipt:
            raise ValueError("发布对象观察期间收据变化")
        return ObjectObservation(resource, directory, None if exists else receipt_sha256)
    except Exception as error:
        write_json(output / "failure.json", {"format_version": 1, "status": "object_observation_failed",
                   "stage": stage, "error_type": type(error).__name__, "binding": initial,
                   "remote_writes": 0, "clone_verified": False, "restore_qualified": False})
        raise ObjectObservationError(output) from None


def verify_target_heads(backend: Path, tools: ExternalTools, items: list[dict], observations: dict,
                        output: Path) -> dict:
    """完整字节核验之后重读当前 HEAD，捕获收尾期间同 key 替换；不下载业务内容。"""
    output = local_path(backend, str(output), new=True)
    work = local_path(backend, str(tools.work))
    if not output.parent.is_dir() or output == work or not output.is_relative_to(work):
        raise ValueError("目标 HEAD 复核须写入本次明确工作目录")
    if len(items) != len(observations) or {(item["bucket"], item["target_key"]) for item in items} != set(observations):
        raise ValueError("目标 HEAD 复核必须覆盖最后完整核验的全部对象")
    output.mkdir()
    try:
        initial, frozen = fixed_tools(tools, output)
        reader = CaptureReader(frozen, "target", output)
        before = reader.owners("owner-before")

        def observe(task):
            index, item = task
            bucket, key = item["bucket"], item["target_key"]
            observed = observations[bucket, key]
            resource = {"kind": "object", "scope_id": tools.plan["target"]["scope_id"],
                        "endpoint": tools.plan["target"]["s3"]["endpoint"], "bucket": bucket, "key": key}
            if (not isinstance(observed, ObjectObservation) or observed.resource != resource
                    or observed.capture_directory is None or observed.absence_evidence_sha256 is not None):
                raise ValueError("收尾缺少本目标真实完整下载的观察")
            expected = {field: item["artifact"][field] for field in ("bytes", "sha256")}
            capture = verify_capture(tools, "target", bucket, key, observed.capture_directory,
                                     expected=expected, max_bytes=expected["bytes"], owner_buckets=(bucket,))["capture"]
            if capture["metadata"] != item["metadata"] or capture["get_header_differences"]:
                raise ValueError("完整对象采集与计划存储或 GET 元数据不同")
            raw = reader.read(f"head-{index}", "head-object", bucket, key, identity=capture["identity"])
            if (inspect_response(raw) != {field: capture[field] for field in ("identity", "metadata")}
                    or plan_hash(raw) != capture["response_sha256"]["head_before"]):
                raise ValueError("目标对象在完整字节核验之后被同 key 替换或元数据变化")
            return {"resource": resource, "head_sha256": plan_hash(raw)}

        results = ordered_batches(observe, list(enumerate(items)), thread_name_prefix="target-head")
        after = reader.owners("owner-after")
        if current_binding(tools) != initial:
            raise ValueError("目标收尾期间计划或认证变化")
        receipt = {"status": "target_object_heads_verified", "objects": results, "binding": initial,
                   "ownership_before": before, "ownership_after": after, "business_downloads": 0,
                   "remote_writes": 0, "restore_qualified": False}
        write_json(output / "verified.json", receipt)
        return receipt
    except BaseException as error:
        write_json(output / "failure.json", {"status": "target_heads_failed", "error_type": type(error).__name__,
                   "remote_writes": 0, "restore_qualified": False})
        raise

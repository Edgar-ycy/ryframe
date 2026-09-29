"""精确只读采集开发复制对象；分别证明存储元数据、字节和 GET 响应契约。"""

from __future__ import annotations

import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess

from artifact_digests import filesystem_path
from restore_build import file_digest
from restore_reference_io import COPY_METADATA_FIELDS, ExternalTools, copy_object_metadata, redact_object_diagnostic
from restore_reference_plan import BUCKETS, plan_hash

OBSERVED_FIELDS = {"AcceptRanges", "ContentLength", "ETag", "LastModified", "VersionId", "StorageClass",
                   "MissingMeta", "TagCount", "PartsCount", "ChecksumCRC32", "ChecksumCRC32C",
                   "ChecksumCRC64NVME", "ChecksumSHA1", "ChecksumSHA256", "ChecksumType"}
OWNER_BUCKETS = tuple(sorted(BUCKETS))


def owner_scope(owner_buckets: tuple[str, ...], *, bucket: str | None = None) -> tuple[str, ...]:
    """可信调用方明确选择完整五桶或当前对象所属桶，不能从收据缩小验证范围。"""
    if (type(owner_buckets) is not tuple or not owner_buckets or any(type(name) is not str for name in owner_buckets)
            or owner_buckets != OWNER_BUCKETS and (len(owner_buckets) != 1 or owner_buckets[0] not in BUCKETS)
            or bucket is not None and bucket not in owner_buckets):
        raise ValueError("ownership 范围必须是完整五桶或当前对象唯一所属桶")
    return owner_buckets


class ObjectCaptureError(RuntimeError):
    def __init__(self, output: Path):
        self.evidence_directory = str(output)
        super().__init__(f"对象只读采集未通过；证据保留于 {output}")


def write_json(path: Path, value: dict) -> None:
    with open(filesystem_path(path), "x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def unique_object(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("对象工具响应包含重复 JSON 字段")
        result[key] = value
    return result


def validate_request(tools: ExternalTools, side: str, bucket: str, key: str, output: Path,
                     expected: dict, max_bytes: int, *, existing: bool = False) -> None:
    if side not in {"source", "target"} or side not in tools.plan:
        raise ValueError("采集必须明确选择计划中的 source 或 target")
    scope = tools.plan[side]["scope_id"]
    if not isinstance(scope, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{2,47}", scope):
        raise ValueError("采集 scope 无效")
    logical = key.removeprefix(scope + "/") if isinstance(key, str) else ""
    if (bucket not in BUCKETS or not isinstance(key, str) or not key.startswith(scope + "/")
            or logical == ".ryframe-owner" or "\\" in logical
            or any(part in {"", ".", ".."} for part in logical.split("/"))
            or any(ord(char) < 32 or ord(char) == 127 for char in logical)
            or len(key.encode("utf-8")) > 1024):
        raise ValueError("采集只接受明确桶内的精确业务 key，不接受 owner 或越界路径")
    if (not isinstance(expected, dict) or set(expected) != {"bytes", "sha256"}
            or type(expected["bytes"]) is not int or expected["bytes"] < 0
            or not isinstance(expected["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", expected["sha256"])
            or type(max_bytes) is not int or max_bytes < expected["bytes"] or max_bytes < 0):
        raise ValueError("采集必须绑定已登记的字节数、SHA256 和明确大小上限")
    for path in (output, *output.parents, tools.work, *tools.work.parents):
        if path.is_symlink() or (path.exists() and getattr(path.lstat(), "st_file_attributes", 0) & 0x400):
            raise ValueError("采集证据目录不能经过链接")
    if (not output.is_absolute() or not tools.work.is_absolute() or not tools.work.is_dir()
            or output.resolve() == tools.work.resolve() or not output.resolve().is_relative_to(tools.work.resolve())
            or not output.parent.is_dir() or output.exists() != existing or existing and not output.is_dir()):
        raise ValueError("采集输出必须是当前 work 内状态符合本次操作的明确独立目录")


def regular_file(path: Path) -> Path:
    for item in (path, *path.parents):
        if item.is_symlink() or (item.exists() and getattr(item.lstat(), "st_file_attributes", 0) & 0x400):
            raise ValueError("采集文件不能经过链接")
    if not path.is_file():
        raise ValueError("采集证据文件缺失")
    return path


def read_json(path: Path) -> dict:
    value = json.loads(regular_file(path).read_text(encoding="utf-8"), object_pairs_hook=unique_object)
    if not isinstance(value, dict):
        raise ValueError("采集证据必须是 JSON 对象")
    return value


def read_bound_json(path: Path, descriptor: dict) -> dict:
    """用同一打开句柄核对文件身份、字节数与摘要后解析唯一 JSON 对象。"""
    expected = copy.deepcopy(descriptor)
    if (not isinstance(descriptor, dict) or set(descriptor) != {"path", "bytes", "sha256"}
            or descriptor["path"] != str(path) or type(descriptor["bytes"]) is not int
            or descriptor["bytes"] <= 0 or not isinstance(descriptor["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", descriptor["sha256"]) is None):
        raise ValueError("绑定 JSON 文件描述无效")
    path = regular_file(path)
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        content = stream.read(16 * 1024 * 1024 + 1)
        after = os.fstat(stream.fileno())
    current = path.stat()
    def identity(value):
        return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns
    if (not stat.S_ISREG(before.st_mode) or identity(before) != identity(after)
            or identity(after) != identity(current) or len(content) != descriptor["bytes"]
            or len(content) > 16 * 1024 * 1024
            or hashlib.sha256(content).hexdigest() != descriptor["sha256"] or descriptor != expected):
        raise ValueError("绑定 JSON 文件身份、字节或摘要变化")
    value = json.loads(content.decode("utf-8"), object_pairs_hook=unique_object)
    if not isinstance(value, dict):
        raise ValueError("绑定 JSON 必须是对象")
    return value


def request_data(tools: ExternalTools, side: str, bucket: str, key: str, expected: dict, max_bytes: int) -> dict:
    config = tools.plan[side]
    return {"format_version": 1, "kind": "devex-clone-object-capture", "side": side,
            "scope_id": config["scope_id"], "endpoint": config["s3"]["endpoint"],
            "region": config["s3"]["region"], "bucket": bucket, "key": key,
            "expected": dict(expected), "max_bytes": max_bytes,
            "aws_tool": tools.plan["tools"]["aws"], "clone_verified": False}


def inspect_response(value: dict) -> dict:
    unknown = set(value) - COPY_METADATA_FIELDS - OBSERVED_FIELDS
    if unknown:
        raise ValueError("对象响应包含未支持的元数据或策略字段：" + ", ".join(sorted(unknown)))
    if ("ContentType" not in value or "Metadata" not in value
            or value.get("MissingMeta", 0) != 0 or value.get("TagCount", 0) != 0
            or value.get("StorageClass", "STANDARD") != "STANDARD"):
        raise ValueError("对象元数据缺失、截断、含未支持标签或非 STANDARD 存储策略")
    size, etag, modified = value.get("ContentLength"), value.get("ETag"), value.get("LastModified")
    if (type(size) is not int or size < 0 or not isinstance(etag, str)
            or not re.fullmatch(r'"[\x21\x23-\x7e]+"', etag) or not isinstance(modified, str)):
        raise ValueError("对象大小、ETag 或修改时间无效")
    instant = dt.datetime.fromisoformat(modified.replace("Z", "+00:00"))
    if instant.tzinfo is None or value.get("AcceptRanges", "bytes") != "bytes":
        raise ValueError("对象修改时间必须带时区且响应必须为完整对象")
    version = value.get("VersionId")
    if "VersionId" in value and (not isinstance(version, str) or not version or any(ord(c) < 32 for c in version)):
        raise ValueError("对象版本标识无效")
    metadata = copy_object_metadata({field: value.get(field) for field in COPY_METADATA_FIELDS})
    return {"metadata": metadata, "identity": {"ContentLength": size, "ETag": etag,
            "LastModified": instant.astimezone(dt.timezone.utc).isoformat(), "VersionId": version}}


def header_differences(head: dict, downloaded: dict) -> list[dict]:
    return [{"field": field, "head_present": field in head, "head_value": head.get(field),
             "get_present": field in downloaded, "get_value": downloaded.get(field)}
            for field in sorted(COPY_METADATA_FIELDS)
            if (field in head, head.get(field)) != (field in downloaded, downloaded.get(field))]


def capture_result(request: dict, before_raw: dict, get_raw: dict, after_raw: dict,
                   actual: dict, owner_before: list, owner_after: list) -> dict:
    before, downloaded, after = map(inspect_response, (before_raw, get_raw, after_raw))
    if (downloaded["identity"] != before["identity"] or actual != request["expected"]
            or actual["bytes"] != before["identity"]["ContentLength"] or actual["bytes"] > request["max_bytes"]):
        raise ValueError("GET 对象身份、完整字节数或 SHA256 与登记值不符")
    if after != before or after_raw != before_raw:
        raise ValueError("采集前后 HEAD 或完整存储元数据变化")
    differences = header_differences(before_raw, get_raw)
    return {**request, "status": "captured", "stored_metadata_verified": True, "bytes_verified": True,
            "get_header_consistent": not differences, "get_header_differences": differences,
            "metadata": before["metadata"], "identity": before["identity"],
            "response_sha256": {"head_before": plan_hash(before_raw), "get": plan_hash(get_raw), "head_after": plan_hash(after_raw)},
            "artifact": {"file": "object.bin", **actual}, "ownership_before": owner_before,
            "ownership_after": owner_after, "request_sha256": plan_hash(request),
            "live_preconditions": "仅证明本次精确对象观察；源停止、完整清单和实际复制仍须上层独立验证"}


def owner_evidence(output: Path, stage: str, bucket: str, scope: str) -> dict:
    body = regular_file(output / f"{stage}-{bucket}.bin")
    expected = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
    if body.stat().st_size != len(expected) or body.read_bytes() != expected:
        raise ValueError("对象 ownership 不属于明确计划 scope")
    return {"bucket": bucket, "key": f"{scope}/.ryframe-owner", "file": body.name, **file_digest(body)}


class CaptureReader:
    def __init__(self, tools: ExternalTools, side: str, output: Path):
        self.tools = ExternalTools(copy.deepcopy(tools.plan), output, tools.run)
        self.side, self.output = side, output

    def read(self, stage: str, operation: str, bucket: str, key: str, *, identity=None, body=None) -> dict:
        if operation not in {"head-object", "get-object"} or (operation == "get-object") != (body is not None):
            raise ValueError("对象采集只能发出完整 HEAD 或 GET 只读请求")
        config, env = self.tools.aws_context(self.side)
        command = [*self.tools.command("aws"), "--endpoint-url", config["endpoint"], "--region", config["region"],
                   "s3api", operation, "--bucket", bucket, "--key", key]
        if identity is not None:
            command.extend(["--if-match", identity["ETag"]])
            if operation == "get-object" and identity["VersionId"] is not None:
                command.extend(["--version-id", identity["VersionId"]])
        if body is not None:
            if body.exists() or body.is_symlink():
                raise ValueError("采集不能覆盖任何已有字节文件")
            command.append(str(body))
        diagnostic = self.output / f"{stage}.diagnostic.json"
        # 在请求前排他创建诊断位置，失败不丢失 CLI 的脱敏原因。
        with diagnostic.open("x", encoding="utf-8", newline="\n") as stream:
            stderr, returncode, error_type = b"", None, None
            try:
                result = self.tools.execute(command, env=env)
                stderr, returncode = result.stderr, result.returncode
                response = json.loads(result.stdout, object_pairs_hook=unique_object)
                if not isinstance(response, dict):
                    raise ValueError("对象工具响应不是 JSON 对象")
                write_json(self.output / f"{stage}.json", response)
                return response
            except (subprocess.SubprocessError, OSError, ValueError) as error:
                stderr = getattr(error, "stderr", stderr)
                returncode, error_type = getattr(error, "returncode", returncode), type(error).__name__
                raise
            finally:
                json.dump({"operation": operation, "returncode": returncode, "error_type": error_type,
                           "stderr": redact_object_diagnostic(stderr, env)}, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())

    def owners(self, stage: str, owner_buckets: tuple[str, ...] = OWNER_BUCKETS) -> list[dict]:
        scope = self.tools.plan[self.side]["scope_id"]
        evidence = []
        for bucket in owner_scope(owner_buckets):
            body = self.output / f"{stage}-{bucket}.bin"
            key = f"{scope}/.ryframe-owner"
            self.read(f"{stage}-{bucket}", "get-object", bucket, key, body=body)
            evidence.append(owner_evidence(self.output, stage, bucket, scope))
        return evidence


def copy_evidence(source: Path, destination: Path, bindings: dict) -> None:
    """只复制本进程真实读取后绑定的小证据；目标排他写入，前后核对原始字节。"""
    for name, expected in bindings.items():
        path = regular_file(source / name)
        if file_digest(path) != expected:
            raise ValueError("对象阶段原始证据在复用前变化")
        content = path.read_bytes()
        with (destination / name).open("xb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        if file_digest(regular_file(destination / name)) != expected or file_digest(path) != expected:
            raise ValueError("对象阶段原始证据在复制期间变化")


class CaptureStages:
    """一次实际读取的阶段状态；不接受外部 JSON 或历史证据作为已验证的起点。"""
    def __init__(self, reader: CaptureReader, bucket: str, key: str, expected: dict, max_bytes: int,
                 *, owner_buckets: tuple[str, ...] = OWNER_BUCKETS):
        self.reader, self.bucket, self.key = reader, bucket, key
        self.owner_buckets = owner_scope(owner_buckets, bucket=bucket)
        self.request = request_data(reader.tools, reader.side, bucket, key, expected, max_bytes)
        self._before, self._owners, self._bindings, self._completed = None, None, {}, False

    def _bind(self, stages: list[str]) -> None:
        for stage in stages:
            names = [f"{stage}.json", f"{stage}.diagnostic.json"]
            if stage.startswith("owner-"):
                names.append(f"{stage}.bin")
            self._bindings.update({name: file_digest(regular_file(self.reader.output / name)) for name in names})

    def owners_before(self) -> list[dict]:
        if self._owners is not None:
            raise ValueError("不能重复开始对象 ownership 阶段")
        self._owners = self.reader.owners("owner-before", self.owner_buckets)
        self._bind([f"owner-before-{bucket}" for bucket in self.owner_buckets])
        return copy.deepcopy(self._owners)

    def head_before(self) -> None:
        if self._owners is None or self._before is not None:
            raise ValueError("对象 HEAD 必须紧接本次 ownership 阶段")
        self._before = self.reader.read("head-before", "head-object", self.bucket, self.key)
        self._bind(["head-before"])

    def complete(self, output: Path) -> dict:
        if self._before is None or self._completed:
            raise ValueError("完整采集必须消费本次真实 HEAD，且只能完成一次")
        self._completed = True
        reader = self.reader
        if output != reader.output:
            validate_request(reader.tools, reader.side, self.bucket, self.key, output,
                             self.request["expected"], self.request["max_bytes"])
            if output.parent != reader.output:
                raise ValueError("复用对象读取只能创建同次观察下的 capture 目录")
            output.mkdir()
            write_json(output / "intent.json", self.request)
            reader = CaptureReader(reader.tools, reader.side, output)
        try:
            if output != self.reader.output:
                copy_evidence(self.reader.output, output, self._bindings)
            result = self._finish(reader)
            if any(file_digest(regular_file(self.reader.output / name)) != expected
                   for name, expected in self._bindings.items()):
                raise ValueError("对象读取期间起始证据变化")
            write_json(output / "capture.json", result)
            if output != self.reader.output:
                names = [f"owner-after-{bucket}.{suffix}" for bucket in self.owner_buckets
                         for suffix in ("bin", "json", "diagnostic.json")]
                copy_evidence(output, self.reader.output, {name: file_digest(regular_file(output / name)) for name in names})
            return result
        except (subprocess.SubprocessError, OSError, ValueError) as error:
            capture_failed(output, self.request, error)

    def _finish(self, reader: CaptureReader) -> dict:
        before_raw, expected, max_bytes = self._before, self.request["expected"], self.request["max_bytes"]
        before = inspect_response(before_raw)
        if before["identity"]["ContentLength"] != expected["bytes"] or before["identity"]["ContentLength"] > max_bytes:
            raise ValueError("HEAD 大小与登记值或采集上限不符")
        body = reader.output / "object.bin"
        get_raw = reader.read("get", "get-object", self.bucket, self.key, identity=before["identity"], body=body)
        downloaded = inspect_response(get_raw)
        actual = file_digest(regular_file(body))
        if downloaded["identity"] != before["identity"] or actual != expected or actual["bytes"] > max_bytes:
            raise ValueError("GET 对象身份、完整字节数或 SHA256 与登记值不符")
        after_raw = reader.read("head-after", "head-object", self.bucket, self.key, identity=before["identity"])
        after = inspect_response(after_raw)
        if after != before or after_raw != before_raw:
            raise ValueError("采集前后 HEAD 或完整存储元数据变化")
        owner_after = reader.owners("owner-after", self.owner_buckets)
        if file_digest(body) != actual:
            raise ValueError("采集期间本地对象字节变化")
        return capture_result(self.request, before_raw, get_raw, after_raw, actual, self._owners, owner_after)


def capture_failed(output: Path, request: dict, error: Exception) -> None:
    write_json(output / "failure.json", {**request, "status": "capture_failed", "error_type": type(error).__name__,
               "reason": str(error) if isinstance(error, ValueError) else "外部只读请求或本地证据写入失败；请核对对应 diagnostic"})
    raise ObjectCaptureError(output) from None


def capture_object(tools: ExternalTools, side: str, bucket: str, key: str, output: Path,
                   *, expected: dict, max_bytes: int, owner_buckets: tuple[str, ...] = OWNER_BUCKETS) -> dict:
    """只捕获已登记对象；大小上限是 HEAD/下载后验证，不是 CLI 硬截断。

    只有完整证据落盘才返回；失败目录不能消费残留 capture.json，GET 头差异始终单列。
    """
    validate_request(tools, side, bucket, key, output, expected, max_bytes)
    owner_scope(owner_buckets, bucket=bucket)
    output.mkdir()
    stages = CaptureStages(CaptureReader(tools, side, output), bucket, key, expected, max_bytes, owner_buckets=owner_buckets)
    write_json(output / "intent.json", stages.request)
    try:
        stages.owners_before()
        stages.head_before()
        return stages.complete(output)
    except (subprocess.SubprocessError, OSError, ValueError) as error:
        capture_failed(output, stages.request, error)


def verify_capture(tools: ExternalTools, side: str, bucket: str, key: str, output: Path,
                   *, expected: dict, max_bytes: int, owner_buckets: tuple[str, ...] = OWNER_BUCKETS) -> dict:
    """只复核已有证据字节，不访问 S3，也不证明此刻 live 源仍相同或复制已成功。"""
    validate_request(tools, side, bucket, key, output, expected, max_bytes, existing=True)
    owner_scope(owner_buckets, bucket=bucket)
    failure = output / "failure.json"
    if failure.exists() or failure.is_symlink():
        raise ValueError("采集目录有失败证据，不能使用残留成功文件")
    tools.command("aws")
    request = request_data(tools, side, bucket, key, expected, max_bytes)
    if read_json(output / "intent.json") != request:
        raise ValueError("采集 intent 与当前明确请求、工具及目标绑定不一致")
    stages = {"head-before": "head-object", "get": "get-object", "head-after": "head-object"}
    owners = {}
    for phase in ("owner-before", "owner-after"):
        owners[phase] = [owner_evidence(output, phase, name, request["scope_id"]) for name in owner_buckets]
        stages.update({f"{phase}-{name}": "get-object" for name in owner_buckets})
    for stage, operation in stages.items():
        diagnostic = read_json(output / f"{stage}.diagnostic.json")
        if (set(diagnostic) != {"operation", "returncode", "error_type", "stderr"}
                or diagnostic["operation"] != operation or type(diagnostic["returncode"]) is not int
                or diagnostic["returncode"] != 0 or diagnostic["error_type"] is not None
                or not isinstance(diagnostic["stderr"], str)):
            raise ValueError("采集只读请求诊断不完整或包含失败")
        read_json(output / f"{stage}.json")
    actual = file_digest(regular_file(output / "object.bin"))
    calculated = capture_result(request, read_json(output / "head-before.json"), read_json(output / "get.json"),
                                read_json(output / "head-after.json"), actual, owners["owner-before"], owners["owner-after"])
    if read_json(output / "capture.json") != calculated:
        raise ValueError("采集结果与原始 HEAD、GET、owner 或实际文件重新计算不符")
    if failure.exists() or failure.is_symlink():
        raise ValueError("采集证据复核期间出现失败记录")
    return {"status": "capture_evidence_verified", "live_revalidated": False, "clone_verified": False,
            "capture": calculated, "capture_file": file_digest(regular_file(output / "capture.json"))}

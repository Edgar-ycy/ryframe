"""在原复制账本内发布冻结 seed 的只读运行后继；原 C52 文件保持不可变。"""
from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response

import copy
from contextlib import contextmanager
import os
from pathlib import Path
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_model import local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, validate_request
from source_fingerprints import (
    current_execution_source,
    require_current_execution_source,
    verify_execution_source,
)

START = "source-generation-start"
STOP = "source-generation-stop"
RECOVER = "source-generation-recover"
STATUS = "source-generation-status"
REQUEST_FIELDS = {"format_version", "kind", "id", "source_registration", "review_successor",
                  "source_rebind", "current_storage", "execution_backend", "expected_backend_sha",
                  "adapter_contract", "product_backend", "backend_build", "maintenance_build", "source_environment"}

# 当前 reboot 与业务证明调用方仍消费这些字段集合；生成主编排的判定由 Rust 执行。
RESULT_FIELDS = {"status", "request", "source_registration", "source_rebind", "review_successor",
                 "current_storage", "history_length", "history_sha256", "source_request",
                 "generation_verified", "runtime_evidence", "before", "after", "remote_writes", "restore_qualified"}
RESULT_FIELDS.add("dataset_lineage")
RESULT_FIELDS.update({"start", "source_runtime", "running", "stop_before"})
START_FIELDS = {"status", "request", "source_registration", "source_rebind", "review_successor", "current_storage",
                "history_length", "history_sha256", "generation_id", "intent", "runtime_evidence", "before", "running",
                "dataset_lineage", "coordinator_source", "remote_writes", "restore_qualified"}


def preflight(
    directory: Path, *, backend: Path | None = None, request_path: Path | None = None
) -> None:
    from devex_clone_seed_generation_prelaunch import closed

    backend = backend or Path(__file__).resolve().parents[2]
    request_descriptor = (
        None
        if request_path is None
        else binding(local_path(backend, str(request_path)))
    )
    attempts = load_state(directory)["attempts"]
    archived = closed(backend, directory, attempts)
    archive = None if archived is None else {
        key: archived[key] for key in ("records", "recovery") if key in archived}
    planned = _generation_model(backend, "preflight-plan", {"attempts": attempts, "archive": archive})
    segment = None
    if planned["needs_segment"]:
        from devex_clone_seed_segment import segmented_resume

        segment = segmented_resume(
            backend, directory, attempts, archived, archived["receipt"]["source_registration"])
    selected = None if segment is None else {
        key: segment[key] for key in ("phase", "storage", "cache", "records", "replay") if key in segment}
    _generation_model(backend, "preflight-finish", {
        "attempts": attempts, "archive": archive, "segment": selected,
        "request": request_descriptor,
        "current_request": None if request_path is None else binding(local_path(backend, str(request_path)))})


def inputs(backend: Path, directory: Path, request_path: Path, number: int) -> tuple[dict, dict, list]:
    from devex_clone_run import _require_owned_run
    from devex_clone_seed_export import _source

    _require_owned_run(directory)
    request = read_json(local_path(backend, str(request_path)))
    state = load_state(directory)
    _generation_model(backend, "input-stage", {
        "request": request, "attempts": state["attempts"], "number": number})
    require_current_execution_source(backend, state["attempts"][-1]["sources"])
    source = _source(backend, directory, live=False)
    checked = _generation_model(backend, "inputs", {
        "request": request, "source": _source_model(source), "attempts": state["attempts"], "number": number})
    for field in ("source_registration", "source_rebind", "review_successor", "backend_build",
                  "source_environment"):
        bound_file(backend, request[field])
    from reference_fixture_successor_generation import rebuild

    if rebuild(backend, request) != request:
        raise ValueError("source-generation 请求与正式生产者重算结果不同")
    return request, source, checked["prefix"]


def source_request(source: dict, request: dict, runtime: Path) -> dict:
    build = read_json(Path(request["backend_build"]["path"]))
    return _generation_model(Path(__file__).resolve().parents[2], "source-request", {
        "source_request": source["request"], "request": request,
        "fingerprint": build["sources"]["full"]["source"]["worktree_fingerprint"],
        "runtime": binding(runtime / "runtime.json"),
        "processes": {role: binding(runtime / f"{role}.json") for role in ("api", "worker")},
        "producers_registry": binding(runtime.parent / "producers.json"), "runtime_dir": str(runtime)})


def predecessor(backend: Path, directory: Path, state: dict) -> dict:
    from devex_clone_seed_source import _registered_source
    from devex_clone_seed_rebind import resolve_storage
    from reference_fixture_successor import _source_with_loader

    record = _generation_model(backend, "select-rebind", {"attempts": state["attempts"]})["record"]
    rebound = read_json(bound_file(backend, record["result"]))
    source = _source_with_loader(backend, rebound["review_successor"], live_storage=False, loader=_registered_source)
    if source["directory"] != directory:
        raise ValueError("源运行代次不属于同一原复制账本")
    return resolve_storage(backend, rebound["source_registration"], source, state, live_storage=False)


def _verify_running_source(
    backend: Path, start_descriptor: dict, *, live: bool, historical_coordinator: bool
) -> dict:
    from devex_clone_seed_generation_images import verify_image
    from devex_clone_seed_generation_runtime import verify_running_evidence

    path = bound_file(backend, start_descriptor)
    directory = path.parent.parent
    state_binding = binding(directory / "state.json")
    state = load_state(directory)
    from devex_clone_seed_generation_prelaunch import starts

    selected = _generation_model(backend, "select-start", {
        "attempts": starts(backend, directory, state["attempts"])})
    records = [selected["record"]]
    if (records[0]["result"] != start_descriptor
            or path != directory / "results" / f"{records[0]['number']:04d}.json"):
        raise ValueError("source verify 必须绑定同一账本唯一成功 start 的外层收据")
    coordinator_source = copy.deepcopy(records[0]["sources"])
    if historical_coordinator:
        verify_execution_source(coordinator_source, "历史 source generation 协调器")
        if coordinator_source["snapshot"]["clean"] is not True:
            raise ValueError("历史 source generation 协调器不是已登记干净来源")
    else:
        coordinator_source = require_current_execution_source(backend, coordinator_source)
    value = read_json(path)
    output = directory / f"g{records[0]['number']:04d}"
    source = predecessor(backend, directory, state)
    request = read_json(bound_file(backend, value["request"]))
    prefix = [row for row in state["attempts"] if row["number"] < records[0]["number"]]
    _generation_model(backend, "running", {
        "receipt": value, "request": request, "source": _source_model(source), "prefix": prefix,
        "coordinator_source": coordinator_source, "generation_id": output.name,
        "request_copy": read_json(output / "request.json"), "attempts": state["attempts"],
        "number": records[0]["number"], "live": live})
    for key, filename in (("intent", "intent.json"), ("runtime_evidence", "running-evidence.json"),
                          ("before", "before/image.json"), ("running", "running/image.json"),
                          ("dataset_lineage", "dataset-lineage.json")):
        if bound_file(backend, value[key]) != output / filename:
            raise ValueError("运行中源证据不属于同一固定代次目录")
    selected = {**copy.deepcopy(source["request"]["source"]), "runtime_dir": str(output / "runtime")}
    images = {key: verify_image(backend, value[key], selected, source["request"],
              source_registration=value["source_registration"]) for key in ("before", "running")}
    if not _images_equal(backend, images["before"]["image"], images["running"]["image"]):
        raise ValueError("运行中源启动前后存在数据库、对象或 Redis 漂移")
    from devex_clone_post_registration import resolve_post_registration
    from devex_clone_run import require_target_copy
    from devex_clone_post import _validate_business_lineage
    lineage_post = resolve_post_registration(backend, source["directory"], descriptor=source["registration"]["post_copy"])
    lineage_copy = require_target_copy(backend, source["directory"], source["manifest"], ("api",)).plan
    _validate_business_lineage(backend, source["manifest"], lineage_post.request, lineage_copy)
    lineage = native_response(*native_invoke(
        "__source-evidence", {"operation": "lineage-verify", "write": False,
            "arguments": {
                'backend': str(backend),
                'source': {**{key: source[key] for key in ("review_successor", "result", "registration", "manifest", "request")}, "directory": str(source["directory"])},
                'post_request': lineage_post.request,
                'post_descriptor': lineage_post.descriptor,
                'copy_plan': lineage_copy,
                'image_descriptor': value["before"],
                'verified_image': images["before"],
                'lineage_descriptor': value["dataset_lineage"],
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    runtime = verify_running_evidence(backend, output, request, selected, source["generation"]["physical_binding"], live=live)
    if live:
        from devex_clone_storage import current_storage_binding

        if current_storage_binding(backend, directory, "target") != value["current_storage"]:
            raise ValueError("源运行中的存储代次变化")
    if binding(path) != start_descriptor or binding(directory / "state.json") != state_binding:
        raise ValueError("源运行收据或账本在复核期间变化")
    if historical_coordinator:
        verify_execution_source(coordinator_source, "历史 source generation 协调器")
    else:
        require_current_execution_source(backend, coordinator_source)
    return {"receipt": value, "start_descriptor": start_descriptor, "directory": directory, "output": output,
            "request": request, "source": source, "execution": Path(request["execution_backend"]), "selected": selected,
            "runtime": runtime, **images, "lineage": lineage, "coordinator_source": coordinator_source,
            "environment": read_json(bound_file(backend, request["source_environment"]))}


def verify_running_source(backend: Path, start_descriptor: dict, *, live: bool) -> dict:
    """普通路径始终要求当前 checkout 等于 START 登记的干净协调器。"""
    return _verify_running_source(
        backend, start_descriptor, live=live, historical_coordinator=False
    )


def verify_historical_running_source(backend: Path, start_descriptor: dict, *, live: bool) -> dict:
    """恢复专用：核验 START 的历史来源，不把它冒充当前 checkout。"""
    return _verify_running_source(
        backend, start_descriptor, live=live, historical_coordinator=True
    )


@contextmanager
def registered_running_source(backend: Path, start_descriptor: dict):
    from devex_clone_run import _require_owned_run
    from devex_clone_run_state import run_lock
    from process_environment import Environments, configured

    initial = verify_running_source(backend, start_descriptor, live=True)
    environments = Environments(dict(os.environ), configured(initial["environment"]["environment"]))
    directory = initial["directory"]
    active = True
    with run_lock(directory):
        lock = directory / "run.lock"
        identity = (lock.stat().st_dev, lock.stat().st_ino)

        def checkpoint():
            from devex_clone_model import linked

            if not active or linked(lock) or (lock.stat().st_dev, lock.stat().st_ino) != identity:
                raise ValueError("源验证控制锁身份变化或 checkpoint 已离开控制区间")
            _require_owned_run(directory)
            if dict(os.environ) not in environments.values.values():
                raise ValueError("源验证控制或服务环境发生变化")
            with environments.use("source"):
                current = verify_running_source(backend, start_descriptor, live=True)
            if current != initial:
                raise ValueError("源验证期间运行代次、来源或前像绑定变化")
            return current

        try:
            checkpoint()
            yield checkpoint
        except BaseException as error:
            try:
                checkpoint()
            except BaseException as failed:
                error.add_note("源验证异常后的代次复核失败：" + type(failed).__name__)
            raise
        else:
            checkpoint()
        finally:
            active = False


@contextmanager
def registered_historical_running_source(backend: Path, start_descriptor: dict):
    """恢复专用锁；历史 START 与当前恢复工具分别固定，普通验证不使用此路径。"""
    from devex_clone_run import _require_owned_run
    from devex_clone_run_state import run_lock
    from process_environment import Environments, configured

    initial = verify_historical_running_source(backend, start_descriptor, live=True)
    recovery_tools = current_execution_source(backend)
    if recovery_tools["snapshot"]["clean"] is not True:
        raise ValueError("来源验收恢复工具必须先提交为干净来源")
    require_current_execution_source(backend, recovery_tools)
    environments = Environments(dict(os.environ), configured(initial["environment"]["environment"]))
    directory = initial["directory"]
    active = True
    with run_lock(directory):
        lock = directory / "run.lock"
        identity = (lock.stat().st_dev, lock.stat().st_ino)

        def checkpoint():
            from devex_clone_model import linked

            if not active or linked(lock) or (lock.stat().st_dev, lock.stat().st_ino) != identity:
                raise ValueError("源验证恢复控制锁身份变化或 checkpoint 已离开控制区间")
            _require_owned_run(directory)
            if dict(os.environ) not in environments.values.values():
                raise ValueError("源验证恢复控制或服务环境发生变化")
            require_current_execution_source(backend, recovery_tools)
            with environments.use("source"):
                current = verify_historical_running_source(backend, start_descriptor, live=True)
            if current != initial:
                raise ValueError("源验证恢复期间历史运行代次、来源或前像绑定变化")
            return current

        try:
            checkpoint()
            yield checkpoint, copy.deepcopy(recovery_tools)
        except BaseException as error:
            try:
                checkpoint()
            except BaseException as failed:
                error.add_note("源验证恢复异常后的代次复核失败：" + type(failed).__name__)
            raise
        else:
            checkpoint()
        finally:
            active = False


def _generation_evidence(backend: Path, operation: str, arguments: dict):
    """只发布受管的只读请求快照；正式账本、意图和运行证据保持原样。"""
    from workspace_directory import WorkspaceDirectory

    with WorkspaceDirectory(dir=backend / ".local-tests/t", prefix="generation-guard-") as temporary:
        directory = Path(temporary)
        path = directory / "parameters.json"
        write_json(path, {"operation": operation, "arguments": arguments})
        try:
            process, response = native_invoke("__seed-evidence", {
                "format_version": 1, "kind": "ryframe-seed-evidence", "run_dir": str(directory),
                "operation": "generation-guard", "arguments": {
                    "backend_root": str(backend), "parameters": binding(path)}},
                subprocess.run, may_write=False)
        except RuntimeError:
            raise ValueError("生成冻结证据核验未完成；保留阶段现场，禁止自动重放") from None
        result = response.get("result") if isinstance(response, dict) else None
        if (process.returncode == 0 and isinstance(response, dict) and set(response) == {"ok", "result"}
                and response["ok"] is True and isinstance(result, dict)
                and set(result) == {"verified", "reason", "remote_writes", "automatic_retry_permitted"}
                and result["verified"] is False and type(result["remote_writes"]) is int
                and result["remote_writes"] == 0 and result["automatic_retry_permitted"] is False):
            messages = {
                "baseline_drift": "未启动恢复的新完整逻辑基线之后存在未知写入；禁止启动",
                "replay_image_drift": "C69 授权后完整逻辑像发生变化；禁止执行 C70 启动",
                "redis_generation_changed": "分段缓存重启改变了配置、端点、二进制或物理目标"}
            if isinstance(result["reason"], str) and result["reason"] in messages:
                raise ValueError(messages[result["reason"]])
        fields = {"verified", "remote_writes", "automatic_retry_permitted"}
        if operation == "coordinator":
            fields.add("value")
        if (process.returncode != 0 or not isinstance(response, dict)
                or set(response) != {"ok", "result"} or response["ok"] is not True
                or not isinstance(result, dict) or set(result) != fields
                or result["verified"] is not True or type(result["remote_writes"]) is not int
                or result["remote_writes"] != 0 or result["automatic_retry_permitted"] is not False):
            raise ValueError("生成冻结请求、成对构建或完整前像无效；保留阶段现场，禁止自动重放")
        if operation == "coordinator":
            value = result["value"]
            if not isinstance(value, dict):
                raise ValueError("生成编排模型结果无效；保留阶段现场，禁止自动重放")
            if "rejected" in value:
                message = value["rejected"]
                if (set(value) != {"rejected"} or not isinstance(message, str)
                        or not message or len(message) > 512 or any(char in message for char in "\r\n\0")):
                    raise ValueError("生成编排模型拒绝结果无效；禁止自动重放")
                raise ValueError(message)
            return value


def _generation_model(backend: Path, action: str, arguments: dict) -> dict:
    value = _generation_evidence(backend, "coordinator", {"action": action, "input": arguments})
    if action.startswith("select-") and (set(value) != {"record", "prefix"}
            or value["record"] is not None and not isinstance(value["record"], dict)
            or not isinstance(value["prefix"], list)):
        raise ValueError("生成阶段选择结果无效；禁止自动重放")
    field = {"preflight-plan": "needs_segment", "preflight-finish": "ready", "inputs": "prefix", "input-stage": "prefix",
             "running": "verified", "published": "verified", "generation-proof": "verified",
             "equal-images": "equal"}.get(action)
    if field is not None and (set(value) != {field}
            or (not isinstance(value[field], list) if field == "prefix" else type(value[field]) is not bool)
            or field in {"ready", "verified"} and value[field] is not True):
        raise ValueError("生成编排模型结果无效；保留阶段现场，禁止自动重放")
    return value


def _source_model(source: dict) -> dict:
    return {key: source.get(key) for key in (
        "review_successor", "source_rebind", "review_successor_binding", "storage", "source_generation")}


def _images_equal(backend: Path, before: dict, after: dict) -> bool:
    return _generation_model(backend, "equal-images", {"before": before, "after": after})["equal"]


def _archived_request(backend: Path, archive: dict, request: dict, segment: dict | None) -> None:
    selected = None if segment is None else {
        key: segment[key] for key in ("phase", "storage", "cache")}
    _generation_evidence(backend, "archived-request", {
        "previous": archive["receipt"]["request"], "request": request, "segment": selected})


def _reregistered_builds(backend: Path, previous: dict, request: dict) -> None:
    _generation_evidence(backend, "builds", {"previous": previous, "request": request})


def _archived_image(archive: dict, current: dict, segment: dict | None, *, backend: Path) -> None:
    if segment is not None and segment.get("reboot") is not None:
        _reboot_image(archive, current, segment)
        return
    selected = None if segment is None else {
        "storage": {"storage": segment["storage"]["storage"]},
        "cache": {"redis": segment["cache"]["redis"]},
        "replay": None if segment.get("replay") is None else {"image": segment["replay"]["image"]}}
    _generation_evidence(backend, "archived-image", {
        "before": archive["image"], "after": current, "segment": selected})


def _reboot_image(archive: dict, current: dict, segment: dict) -> None:
    """主机重启后的新代次：只允许声明的会话写入，缓存身份按新代次归一化。"""

    expected = copy.deepcopy(archive["image"])
    expected["storage"] = {"rustfs": copy.deepcopy(segment["storage"]["storage"]),
                           "redis": copy.deepcopy(segment["cache"]["redis"])}
    native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'write-effects',
            "write": False,
            "arguments": {
                'before': expected,
                'after': current,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]


def execute_generation(backend: Path, directory: Path, request_path: Path, number: int, *, run=subprocess.run) -> dict:
    from devex_clone_seed_generation_runtime import GenerationRuntime
    from devex_clone_seed_generation_images import capture_image, verify_image
    from devex_clone_seed_generation_prelaunch import closed

    original = inputs(backend, directory, request_path, number)
    request, source, prefix = original
    archive = closed(backend, directory, prefix)
    segment = None
    if archive is not None:
        from devex_clone_seed_segment import segmented_resume

        segment = segmented_resume(backend, directory, prefix, archive, request["source_registration"])
        _archived_request(backend, archive, request, segment)
        if segment is not None and segment.get("replay") is not None:
            if binding(request_path) != segment["replay"]["request"]:
                raise ValueError("C70 必须使用 C69 授权时绑定的同一正式请求文件")
    coordinator_source = require_current_execution_source(backend, load_state(directory)["attempts"][-1]["sources"])
    descriptor = binding(request_path)
    output = local_path(backend, str(directory / f"g{number:04d}"), new=True)
    runtime = GenerationRuntime(backend, directory, output, request, source, run)

    def checkpoint():
        with runtime.control_environment():
            require_current_execution_source(backend, coordinator_source)
            if inputs(backend, directory, request_path, number) != original or binding(request_path) != descriptor:
                raise ValueError("source-generation 执行期间来源、请求或账本前缀发生变化")
        runtime.checkpoint()

    runtime.preflight()
    output.mkdir()
    write_json(output / "request.json", request)
    with runtime.environment() as environment:
        runtime.prepare()
        checkpoint()
        before = capture_image(backend, runtime.execution, runtime.selected, request, source,
                               environment, output / "before", run, control_environment=runtime.control_environment)
        if archive is not None:
            _archived_image(archive, read_json(Path(before["path"]))["image"], segment, backend=backend)
        from devex_clone_post_registration import resolve_post_registration
        from devex_clone_run import require_target_copy
        from devex_clone_post import _validate_business_lineage
        lineage_post = resolve_post_registration(backend, source["directory"], descriptor=source["registration"]["post_copy"])
        lineage_copy = require_target_copy(backend, source["directory"], source["manifest"], ("api",)).plan
        _validate_business_lineage(backend, source["manifest"], lineage_post.request, lineage_copy)
        lineage = native_response(*native_invoke(
            "__source-evidence", {"operation": "lineage-derive", "write": False,
                "arguments": {
                    'backend': str(backend),
                    'source': {**{key: source[key] for key in ("review_successor", "result", "registration", "manifest", "request")}, "directory": str(source["directory"])},
                    'post_request': lineage_post.request,
                    'post_descriptor': lineage_post.descriptor,
                    'copy_plan': lineage_copy,
                    'image_descriptor': before,
                    'verified_image': verify_image(backend, before, runtime.selected, source["request"], source_registration=request["source_registration"]),
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
        write_json(output / "dataset-lineage.json", lineage)
        try:
            checkpoint()
            runtime.start(checkpoint)
            checkpoint()
            running = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                    environment, output / "running", run, control_environment=runtime.control_environment)
            if not _images_equal(backend, read_json(Path(before["path"]))["image"], read_json(Path(running["path"]))["image"]):
                raise ValueError("源启动完整数据库、保留表、placement、对象或 Redis 前后像变化；禁止发布或重放")
            runtime.running_evidence()
            checkpoint()
            receipt = _generation_model(backend, "start-receipt", {
                "request": request, "descriptor": descriptor, "coordinator_source": coordinator_source,
                "prefix": prefix, "generation_id": output.name, "bindings": {
                    "intent": binding(output / "intent.json"),
                    "runtime_evidence": binding(output / "running-evidence.json"), "before": before, "running": running,
                    "dataset_lineage": binding(output / "dataset-lineage.json")}})
            runtime.retain()
        except BaseException as failure:
            try:
                runtime.stop()
                checkpoint()
                after = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                      environment, output / "after", run, control_environment=runtime.control_environment)
                if not _images_equal(backend, read_json(Path(before["path"]))["image"], read_json(Path(after["path"]))["image"]):
                    failure.add_note("失败启动完整后像发生变化，禁止重放")
            except BaseException as error:
                failure.add_note("源代次停止或失败后像复核失败：" + type(error).__name__)
            raise
    return receipt


def resolve_generation(backend: Path, descriptor: dict, source: dict, state: dict, *, live: bool) -> dict:
    """只采用唯一成功外层发布；预览验证全部文件绑定，不启动进程或新建证据。"""
    from devex_clone_seed_source import _validate_evidence_bindings

    result = copy.deepcopy(source)
    result.update(source_request=source["registration"]["source_request"],
                  source_environment=source["registration"]["source_environment"],
                  generation_verified=source["registration"]["generation_verified"], source_generation=None)
    selected = _generation_model(backend, "select-published", {"attempts": state["attempts"]})
    record = selected["record"]
    if record is None:
        return result
    from devex_clone_seed_generation_runtime import verify_runtime_evidence
    path = bound_file(backend, record["result"])
    directory = source["directory"]
    if path != directory / "results" / f"{record['number']:04d}.json":
        raise ValueError("source-generation 外层结果不属于固定 attempt")
    value = read_json(path)
    _validate_evidence_bindings(backend, value)
    from restore_source_runtime import verify_source_runtime

    verified = verify_source_runtime(backend, value["source_runtime"], live=False)
    facts = verified["facts"]
    request = facts["request"]
    prefix = selected["prefix"]
    _generation_model(backend, "published", {
        "receipt": value, "request": request, "source": {**source, "directory": str(source["directory"])},
        "descriptor": descriptor, "prefix": prefix, "source_receipt": facts["receipt"],
        "verified_source": {**facts["source"], "directory": str(facts["source"]["directory"])},
        "verified_receipt": verified["receipt"],
        "before": facts["receipt"]["before"], "running": facts["receipt"]["running"]})
    output = facts["output"]
    for key, filename in (("source_request", "source-request.json"), ("generation_verified", "generation-verified.json"),
                          ("runtime_evidence", "runtime-evidence.json"), ("before", "before/image.json"),
                          ("running", "running/image.json"), ("stop_before", "stop-before/image.json"), ("after", "after/image.json")):
        if bound_file(backend, value[key]) != output / filename:
            raise ValueError("source-generation 证据不属于同一固定代次目录")
    if read_json(output / "request.json") != request:
        raise ValueError("source-generation 请求副本与原绑定不同")
    effective = read_json(bound_file(backend, value["source_request"]))
    validate_request(backend, effective)
    if effective != source_request(source, request, output / "runtime"):
        raise ValueError("后继 source request 改变了同一物理 seed 来源或构建绑定")
    from devex_clone_seed_generation_images import verify_image

    stop_before, after = (verify_image(backend, value[key], effective["source"], source["request"],
                         source_registration=descriptor) for key in ("stop_before", "after"))
    if not _images_equal(backend, stop_before["image"], after["image"]) or not _images_equal(backend, stop_before["image"], verified["after"]["image"]):
        raise ValueError("source verify 后或停止期间完整前后像发生未知变化")
    runtime = verify_runtime_evidence(backend, output, request, effective, live=live)
    generation = read_json(bound_file(backend, value["generation_verified"]))
    _generation_model(backend, "generation-proof", {
        "effective": effective, "runtime": read_json(bound_file(backend, effective["runtime"])),
        "source_snapshot": read_json(bound_file(backend, effective["backend_build"]))["sources"]["full"]["source"]["snapshot"],
        "maintenance": read_json(bound_file(facts["execution"], effective["maintenance_build"])),
        "processes": {role: read_json(bound_file(backend, effective["processes"][role]))["identity"] for role in ("api", "worker")},
        "producers": read_json(bound_file(backend, effective["producers_registry"]))["processes"],
        "physical_binding": source["generation"]["physical_binding"], "generation": generation,
        "runtime_physical": runtime["physical_binding"]})
    result.update(request=effective, source_request=value["source_request"], generation=generation,
                  generation_verified=value["generation_verified"], source_generation=record["result"],
                  source_environment=request["source_environment"],
                  environment=read_json(bound_file(backend, request["source_environment"])))
    return result

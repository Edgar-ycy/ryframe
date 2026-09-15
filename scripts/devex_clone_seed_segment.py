"""只读核验封存 seed 来源的唯一分段资源续作。"""
from __future__ import annotations

import hashlib
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, require_recorded_producer_stopped
from devex_clone_storage import registered_storage_binding
from devex_clone_storage_request import directory_identity
from restore_reference_plan import plan_hash


_CACHE_STOP_FIELDS = {"status", "side", "request", "processes", "copy_usable", "remote_writes",
                      "resources_deleted"}
_STORAGE_STOP_FIELDS = {"status", "side", "request", "processes", "reconciliation_required",
                        "object_api_calls", "database_calls", "resources_deleted"}
_FAILED_STOP_CALLS = (
    ("scripts/devex_clone_run.py", "execute"),
    ("scripts/devex_clone_storage.py", "execute_storage"),
    ("scripts/devex_clone_storage.py", "observe_or_stop"),
    ("scripts/devex_clone_storage_process.py", "stop_record"),
    ("scripts/devex_clone_source_proof.py", "require_closed_port"),
    ("scripts/process_sockets.py", "verify_windows_port_idle"),
)


def _attempt_result(backend: Path, directory: Path, attempt: dict) -> dict:
    path = bound_file(backend, attempt["result"])
    if path != directory / "results" / f"{attempt['number']:04d}.json":
        raise ValueError("分段续作结果不属于相邻账本阶段")
    return read_json(path)


def _same_product_source(reference: dict, attempt: dict) -> None:
    from source_fingerprints import verify_execution_source

    verify_execution_source(reference, "封存阶段")
    verify_execution_source(attempt["sources"], "分段续作阶段")
    if attempt["sources"]["fingerprints"]["product"] != reference["fingerprints"]["product"]:
        raise ValueError("分段续作改变了 seed 产品来源")


def _require_clean_source(source: dict) -> None:
    from source_fingerprints import verify_execution_source

    verify_execution_source(source, "分段续作阶段")
    snapshot = source["snapshot"]
    if (not snapshot["clean"] or snapshot["files"]
            or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()):
        raise ValueError("分段资源停机必须来自完整干净源码")


def _stopped_cache_process(backend: Path, process: dict, attempt: int,
                           runtime: dict, outputs: tuple[Path, ...]) -> None:
    if process.get("state") == "stopped":
        exact(process, {"state", "launcher_alive", "linux", "tools_sha256", "evidence"})
        linux = process["linux"]
        exact(linux, {"alive", "boot_id", "identity", "terminated"})
        if process["launcher_alive"] is not False or linux["alive"] is not False or type(linux["terminated"]) is not bool:
            raise ValueError("缓存停止结果仍包含存活进程")
        path = bound_file(backend, process["evidence"])
        if path not in {output / f"predecessor-{attempt:04d}.json" for output in outputs}:
            raise ValueError("缓存停止结果引用其他运行或代次的 predecessor")
        proof = read_json(path)
        exact(proof, {"process_sha256", "runtime_sha256", "observation"})
        if (process != {**proof["observation"], "evidence": binding(path)}
                or proof["runtime_sha256"] != plan_hash(runtime)):
            raise ValueError("缓存停止结果没有绑定该创建代次的冻结运行像")
        return
    exact(process, {"status", "alive", "boot_id", "identity", "resources_deleted", "runtime", "terminated"})
    if (process["status"] != "redis_process_stopped" or process["alive"] is not False
            or process["resources_deleted"] is not False or type(process["terminated"]) is not bool
            or process["runtime"] != runtime or process["identity"] != runtime.get("linux_identity")
            or process["boot_id"] != runtime.get("boot_id")):
        raise ValueError("缓存停止结果没有完整关闭当前代次")


def _verify_cache_stop(backend: Path, directory: Path, attempts: list, attempt: dict,
                       source_result: dict, successor: dict) -> dict:
    from devex_clone_cache import registration, starts

    value = _attempt_result(backend, directory, attempt)
    exact(value, _CACHE_STOP_FIELDS)
    request, descriptor = registration(backend, directory)
    if (value["status"] != "cache_stopped" or value["side"] != "target" or value["request"] != descriptor
            or value["copy_usable"] is not False or type(value["remote_writes"]) is not int
            or value["remote_writes"] != 0 or value["resources_deleted"] is not False
            or not isinstance(value["processes"], list) or not value["processes"]):
        raise ValueError("分段续作缺少完整缓存停止结果")
    generations = starts(backend, directory, request, descriptor, before=attempt["number"],
                         frozen=(source_result, successor))
    expected = [row[0]["number"] for row in generations]
    observed = [row["attempt"] for row in value["processes"]]
    if observed != expected or len(set(observed)) != len(observed):
        raise ValueError("缓存停止结果没有覆盖全部已登记创建代次")
    outputs = tuple(row[1] for row in generations)
    for row, (_, _, runtime) in zip(value["processes"], generations, strict=True):
        exact(row, {"attempt", "process"})
        _stopped_cache_process(backend, row["process"], row["attempt"], runtime, outputs)
    return {"request": descriptor, "result": value}


def _verify_storage_stop(backend: Path, directory: Path, attempt: dict) -> dict:
    from devex_clone_storage import previous_attempts, registration

    value = _attempt_result(backend, directory, attempt)
    exact(value, _STORAGE_STOP_FIELDS)
    request, descriptor = registration(backend, directory, "target")
    if (value["status"] != "storage_stopped" or value["side"] != "target" or value["request"] != descriptor
            or value["reconciliation_required"] is not True or type(value["object_api_calls"]) is not int
            or value["object_api_calls"] != 0 or type(value["database_calls"]) is not int
            or value["database_calls"] != 0 or value["resources_deleted"] is not False):
        raise ValueError("分段续作缺少零数据操作的完整存储停止结果")
    previous = previous_attempts(backend, directory, "target", descriptor, before=attempt["number"])
    if len(previous) != len(value["processes"]) or not previous:
        raise ValueError("存储停止结果没有覆盖全部已登记创建代次")
    for (record, observed), row in zip(previous, value["processes"], strict=True):
        exact(row, {"attempt", "state", "identity", "terminated", "process_receipt"})
        if (observed["state"] != "recorded" or row["attempt"] != record["number"]
                or row["state"] != "stopped" or row["identity"] != observed["identity"]
                or row["process_receipt"] != observed["process_receipt"] or type(row["terminated"]) is not bool):
            raise ValueError("存储停止结果仍有未知或存活创建代次")
        require_recorded_producer_stopped(row["identity"])
    directory_identity(backend, request["data_directory"])
    return {"request": descriptor, "result": value}


def _verify_failed_storage_stop(backend: Path, directory: Path, attempt: dict,
                                previous: dict, first_storage: int) -> tuple[dict, dict]:
    from devex_clone_storage import controller_binding

    if (attempt["status"] != "failed" or attempt["result"] is not None
            or attempt["error_type"] != "ValueError" or attempt["sources"] != previous["sources"]):
        raise ValueError("分段续作只接受同一来源的固定存储停止失败")
    controller = controller_binding(backend, directory, attempt)
    failure_path = local_path(backend, str(directory / f"failure-{attempt['number']:04d}.json"))
    failure_descriptor = binding(failure_path)
    failure = read_json(bound_file(backend, failure_descriptor))
    exact(failure, {"format_version", "kind", "attempt", "stage", "mode", "error_type", "frames", "controller"})
    frames = failure["frames"]
    calls = tuple((frame.get("file"), frame.get("function")) for frame in frames)
    if (failure["format_version"] != 1 or failure["kind"] != "devex-stage-failure"
            or failure["attempt"] != attempt["number"] or failure["stage"] != "storage-target"
            or failure["mode"] != "stop" or failure["error_type"] != "ValueError"
            or calls != _FAILED_STOP_CALLS
            or any(set(frame) != {"file", "function", "line"} or type(frame["line"]) is not int
                   or frame["line"] <= 0 for frame in frames)
            or failure["controller"] != controller):
        raise ValueError("存储停止失败不是冻结的端口核验调用链")
    controller_value = read_json(bound_file(backend, controller))
    owner = controller_value["owner"]
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    exact(owner["identity"], {"pid", "started", "executable"})
    identity = owner["identity"]
    if (type(controller_value["format_version"]) is not int or type(owner["format_version"]) is not int
            or owner["format_version"] != 1 or owner["directory"] != str(directory)
            or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]
            or type(identity["pid"]) is not int or identity["pid"] <= 1
            or not isinstance(identity["started"], str) or not identity["started"].isdigit()
            or not isinstance(identity["executable"], str) or not Path(identity["executable"]).is_absolute()):
        raise ValueError("失败存储停止的控制器创建身份不完整")
    require_recorded_producer_stopped(identity)
    snapshot = attempt["sources"]["snapshot"]
    if (not snapshot["clean"] or snapshot["files"]
            or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()):
        raise ValueError("存储停止失败不属于封存时的完整干净来源")
    output = local_path(backend, str(directory / "storage-target" / f"a{attempt['number']:04d}"))
    children = list(output.iterdir()) if output.is_dir() and not linked(output) else []
    expected = output / f"p{first_storage:04d}"
    if (children != [expected] or linked(expected) or not expected.is_dir() or any(expected.iterdir())):
        raise ValueError("失败存储停止留下了未知输出或触碰了其他代次")
    return failure_descriptor, controller


def _owned_segment_attempt(directory: Path, attempts: list, attempt: dict, current: int | None) -> None:
    if (current != attempt["number"] or attempt != attempts[-1] or attempt["status"] != "running"
            or attempt["result"] is not None or attempt["error_type"] is not None):
        raise ValueError("分段续作控制动作不是当前唯一持锁阶段")
    from devex_clone_run import _require_owned_run

    _require_owned_run(directory)


def _verify_generation_tail(tail: list[dict], previous: int) -> dict | None:
    if not tail:
        return None
    if any(row["number"] != previous + offset for offset, row in enumerate(tail, 1)):
        raise ValueError("分段资源续作后的生成阶段不相邻")
    operations = [(row["stage"], row["mode"]) for row in tail]
    start, stop, recover = (("seed-runtime", mode) for mode in (
        "source-generation-start", "source-generation-stop", "source-generation-recover"))
    export, reconcile = (("seed-runtime", mode) for mode in (
        "source-export", "source-export-reconcile"))
    if operations[0] != start or operations.count(start) != 1:
        raise ValueError("分段资源恢复后只允许唯一实际 START")
    if len(tail) == 1:
        return None
    second = operations[1]
    if second == recover:
        if tail[0]["status"] == "passed" or len(tail) != 2:
            raise ValueError("成功 START 不能恢复，恢复后也不能重放")
        return None
    if second != stop or tail[0]["status"] != "passed":
        raise ValueError("只有成功 START 才能进入唯一 STOP")
    if len(tail) == 2:
        return None
    third = operations[2]
    if third == recover:
        if tail[1]["status"] == "passed" or len(tail) != 3:
            raise ValueError("成功 STOP 不能恢复，恢复后也不能重放")
        return None
    if third != export or tail[1]["status"] != "passed":
        raise ValueError("只有成功 STOP 才能进入唯一 source-export")
    following = tail[3:]
    if not following:
        return tail[2] if tail[2]["status"] == "passed" else None
    if tail[2]["status"] != "failed" or any((row["stage"], row["mode"]) != reconcile
                                               for row in following):
        raise ValueError("成功或未收尾导出后不能重复导出")
    terminal = [row for row in following if row["status"] != "failed"]
    if len(terminal) > 1 or terminal and terminal[0] != following[-1]:
        raise ValueError("失败导出只允许一个最终 reconcile")
    return terminal[0] if terminal and terminal[0]["status"] == "passed" else None


def _verify_cleanup(backend: Path, directory: Path, attempts: list, cleanup: list[dict],
                    exported: dict | None, reference: dict, descriptor: dict, successor: dict,
                    cache_request: dict, storage_request: dict, current: int | None) -> str:
    if (exported is None or len(cleanup) not in {1, 2}
            or cleanup[0]["number"] != exported["number"] + 1
            or (cleanup[0]["stage"], cleanup[0]["mode"]) != ("cache-target", "stop")):
        raise ValueError("成功导出后必须先执行唯一缓存停机")
    for offset, row in enumerate(cleanup):
        if (row["number"] != cleanup[0]["number"] + offset
                or row["sources"] != exported["sources"]):
            raise ValueError("导出后的资源停机必须相邻且使用同一完整来源")
        _same_product_source(reference, row)
        _require_clean_source(row["sources"])
    cache_stop = cleanup[0]
    if cache_stop["status"] == "running":
        _owned_segment_attempt(directory, attempts, cache_stop, current)
        if len(cleanup) != 1:
            raise ValueError("缓存停机未完成前不能继续存储停机")
        return "cache-stopping"
    if cache_stop["status"] != "passed" or cache_stop["error_type"] is not None:
        raise ValueError("导出后的缓存停机失败或未收尾")
    closed_cache = _verify_cache_stop(
        backend, directory, attempts, cache_stop, descriptor, successor)
    if closed_cache["request"] != cache_request:
        raise ValueError("导出后缓存停机没有消费相同请求")
    if len(cleanup) == 1:
        return "cache-stopped"
    storage_stop = cleanup[1]
    if (storage_stop["stage"], storage_stop["mode"]) != ("storage-target", "stop"):
        raise ValueError("缓存停机后必须立即执行唯一存储停机")
    if storage_stop["status"] == "running":
        _owned_segment_attempt(directory, attempts, storage_stop, current)
        return "storage-stopping"
    if storage_stop["status"] != "passed" or storage_stop["error_type"] is not None:
        raise ValueError("导出后的存储停机失败或未收尾")
    closed_storage = _verify_storage_stop(backend, directory, storage_stop)
    if closed_storage["request"] != storage_request:
        raise ValueError("导出后存储停机没有消费相同请求")
    return "closed"


def segmented_resume(backend: Path, directory: Path, attempts: list, archive: dict,
                     descriptor: dict, *, current: int | None = None) -> dict | None:
    """识别唯一封存前像后的固定停机、RustFS 重启及 Redis 重启序列。"""
    index = attempts.index(archive["recovery"])
    suffix = attempts[index + 1:]
    if not suffix:
        return None
    if (suffix[0]["stage"], suffix[0]["mode"]) != ("cache-target", "stop"):
        if any((row["stage"], row["mode"]) == ("cache-target", "stop") for row in suffix):
            raise ValueError("封存前像后的缓存停机不是首个相邻阶段")
        return None
    expected = (("cache-target", "stop", "passed"), ("storage-target", "stop", "failed"),
                ("storage-target", "stop", "passed"))
    if (len(suffix) < len(expected) or any(row["number"] != archive["recovery"]["number"] + offset
            or tuple(row.get(key) for key in ("stage", "mode", "status")) != shape
            for offset, (row, shape) in enumerate(zip(suffix, expected, strict=False), 1))):
        raise ValueError("封存前像后的分段停机历史不完整或不相邻")
    reference = archive["recovery"]["sources"]
    if suffix[0]["sources"] != reference or suffix[1]["sources"] != reference:
        raise ValueError("缓存停止和首次存储停止必须使用封存时的完整工具来源")
    for row in suffix[:3]:
        _same_product_source(reference, row)
        _require_clean_source(row["sources"])
    cache = _verify_cache_stop(backend, directory, attempts, suffix[0], descriptor,
                               archive["receipt"]["review_successor"])
    storage = _verify_storage_stop(backend, directory, suffix[2])
    failure = _verify_failed_storage_stop(backend, directory, suffix[1], suffix[0],
                                          storage["result"]["processes"][0]["attempt"])
    if cache["request"] == storage["request"]:
        raise ValueError("缓存与对象存储登记意外共享请求")
    phase, current_storage, current_cache = "stopped", None, None
    rest = suffix[3:]
    if rest:
        restart = rest[0]
        _same_product_source(reference, restart)
        _require_clean_source(restart["sources"])
        if (restart["number"] != suffix[2]["number"] + 1
                or (restart["stage"], restart["mode"]) != ("storage-target", "restart")):
            raise ValueError("分段停机后必须先执行唯一对象存储重启")
        if restart["status"] == "running":
            _owned_segment_attempt(directory, attempts, restart, current)
            if len(rest) != 1:
                raise ValueError("未完成对象存储重启后出现其他阶段")
            phase = "storage-running"
        elif restart["status"] == "passed" and restart["error_type"] is None:
            current_storage = registered_storage_binding(
                backend, directory, "target", before=restart["number"] + 1)
            if (current_storage is None or current_storage["attempt"] != restart["number"]
                    or current_storage["restart_result"] != restart["result"]):
                raise ValueError("分段对象存储重启没有绑定同一成功阶段")
            from devex_clone_seed_rebind import transition

            transition(backend, archive["receipt"]["current_storage"], current_storage)
            phase = "storage-ready"
        else:
            raise ValueError("分段对象存储重启失败或未收尾")
    if len(rest) > 1:
        cache_restart = rest[1]
        _same_product_source(reference, cache_restart)
        _require_clean_source(cache_restart["sources"])
        if (phase != "storage-ready" or cache_restart["number"] != rest[0]["number"] + 1
                or (cache_restart["stage"], cache_restart["mode"]) != ("cache-target", "restart")):
            raise ValueError("对象存储后必须执行唯一缓存重启")
        from devex_clone_cache import registered_cache_binding, seed_history_proof

        if cache_restart["status"] not in {"running", "passed"} or (
                cache_restart["status"] == "passed" and cache_restart["error_type"] is not None):
            raise ValueError("分段缓存重启失败或未收尾")
        successor = seed_history_proof(
            backend, directory, cache_restart, descriptor,
            frozen_successor=(archive["receipt"]["review_successor"]
                              if cache_restart["status"] == "passed" else None))
        if successor != archive["receipt"]["review_successor"]:
            raise ValueError("缓存重启没有继承封存请求的同一 successor")
        if cache_restart["status"] == "running":
            _owned_segment_attempt(directory, attempts, cache_restart, current)
            if len(rest) != 2:
                raise ValueError("未完成缓存重启后出现其他阶段")
            phase = "cache-running"
        else:
            current_cache = registered_cache_binding(
                backend, directory, before=cache_restart["number"] + 1,
                frozen=(descriptor, archive["receipt"]["review_successor"]))
            if (current_cache["attempt"] != cache_restart["number"]
                    or current_cache["restart_result"] != cache_restart["result"]
                    or current_cache["request"] != cache["request"]):
                raise ValueError("分段缓存重启没有绑定同一成功阶段和固定请求")
            phase = "ready"
    tail = rest[2:]
    if tail and phase != "ready":
        raise ValueError("分段资源恢复完成前不能执行生成、发布或其他控制动作")
    cleanup_at = next((index for index, row in enumerate(tail)
                       if row["stage"] in {"cache-target", "storage-target"}), len(tail))
    lifecycle, cleanup = tail[:cleanup_at], tail[cleanup_at:]
    for row in lifecycle:
        _same_product_source(reference, row)
        _require_clean_source(row["sources"])
    exported = _verify_generation_tail(
        lifecycle, rest[1]["number"] if len(rest) > 1 else suffix[2]["number"])
    cleanup_records = ()
    if cleanup:
        phase = _verify_cleanup(
            backend, directory, attempts, cleanup, exported, reference, descriptor,
            archive["receipt"]["review_successor"], cache["request"], storage["request"], current)
        cleanup_records = tuple(cleanup)
    if binding(Path(failure[0]["path"])) != failure[0] or binding(Path(failure[1]["path"])) != failure[1]:
        raise ValueError("分段续作核验期间失败或控制器证据变化")
    return {"phase": phase, "records": tuple(suffix[:3 + min(len(rest), 2)]) + cleanup_records,
            "storage": current_storage, "cache": current_cache}

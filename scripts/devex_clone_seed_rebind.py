"""冻结 seed 发布后的存储代次交接；仅追加收据，不改写来源或执行数据操作。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, require_closed_port, require_recorded_producer_stopped
from devex_clone_storage import current_storage_binding, registered_storage_binding
from devex_clone_storage_request import directory_identity
from full_stack_process import process_identity
from restore_reference_plan import plan_hash

FIELDS = {"status", "source_registration", "review_successor", "original_storage", "current_storage",
          "history_length", "history_sha256", "remote_writes", "restore_qualified"}
READ_ONLY_AFTER = {("seed-runtime", "arm-input"), ("storage-target", "status"),
                   ("seed-runtime", "source-export"), ("seed-runtime", "source-export-reconcile")}
BEFORE_REBIND = READ_ONLY_AFTER - {("seed-runtime", "source-export"), ("seed-runtime", "source-export-reconcile")} | {
    ("storage-target", "restart"), ("storage-target", "stop"), ("storage-target", "recover"),
    ("seed-runtime", "stop"), ("seed-runtime", "recover"),
}


def history(directory: Path, state: dict, descriptor: dict, *, current: int | None = None) -> list[dict]:
    attempts = state["attempts"]
    published = [item for item in attempts if item["stage"] == "seed-runtime"
                 and item["mode"] == "source-register" and item["result"] == descriptor
                 and item["status"] == "passed"]
    if len(published) != 1:
        raise ValueError("重绑定必须继承唯一已发布 seed 源")
    later = [item for item in attempts if item["number"] > published[0]["number"]]
    rebound, exported, export_origin = False, False, None
    for item in later:
        operation = (item["stage"], item["mode"])
        if operation == ("seed-runtime", "source-export"):
            if not rebound or exported:
                raise ValueError("source-export 只允许在重绑定后执行并发布一次")
            exported = True
            export_origin = item
        if operation == ("seed-runtime", "source-export-reconcile") and not exported:
            raise ValueError("source-export-reconcile 必须继承原导出阶段")
        if item["number"] == current:
            from devex_clone_run import _require_owned_run

            allowed = READ_ONLY_AFTER if rebound else BEFORE_REBIND | {("seed-runtime", "source-rebind")}
            if item != attempts[-1] or item["status"] != "running" or operation not in allowed:
                raise ValueError("重绑定核验只能排除当前持锁阶段")
            _require_owned_run(directory)
            continue
        if operation == ("seed-runtime", "source-rebind"):
            if rebound or item["status"] != "passed":
                raise ValueError("seed 源存在重复、冲突或未收尾重绑定")
            rebound = True
        elif operation not in (READ_ONLY_AFTER if rebound else BEFORE_REBIND):
            raise ValueError("seed 发布后出现未知写入或重绑定后存储再次换代")
        if item["status"] != "passed":
            if operation in {("seed-runtime", "source-export"),
                             ("seed-runtime", "source-export-reconcile")} and export_origin is not None:
                from devex_clone_seed_export import reconciles_failed_export

                if reconciles_failed_export(directory, state, export_origin, current, descriptor):
                    continue
            raise ValueError("seed 发布后存在失败或未收尾阶段")
    return later


def quiet_producers(backend: Path, source: dict) -> None:
    from devex_clone_seed_source import _validate_evidence_bindings

    lineage = read_json(bound_file(backend, source["registration"]["producer_lineage"]))
    _validate_evidence_bindings(backend, lineage)
    for item in lineage["identities"]:
        require_recorded_producer_stopped(item["identity"])
    runtime = read_json(bound_file(backend, source["request"]["runtime"]))
    require_closed_port(source["request"]["source"]["api_url"])
    require_closed_port(runtime["worker_ready_url"])


def transition(backend: Path, original: dict, current: dict | None) -> None:
    if current is None or current == original:
        raise ValueError("重绑定需要不同的已发布存储代次")
    for field in ("request", "data_directory", "api_url", "console_url"):
        if current[field] != original[field]:
            raise ValueError("重绑定不得改变固定请求、目录身份或监听端点")
    directory_identity(backend, original["data_directory"])
    if (current["storage"]["sha256"] != original["storage"]["sha256"]
            or current["storage"]["identity"]["executable"] != original["storage"]["identity"]["executable"]
            or current["attempt"] <= original["attempt"]):
        raise ValueError("重绑定不是同一二进制的后续代次")
    if process_identity(original["storage"]["identity"]["pid"]) is not None:
        raise ValueError("冻结存储旧进程仍存在或 PID 被复用")
    bound_file(backend, original["request"])
    bound_file(backend, original["restart_result"])
    bound_file(backend, current["restart_result"])


def _active_read(state: dict, directory: Path) -> int | None:
    running = [item for item in state["attempts"] if item["status"] == "running"]
    if not running:
        return None
    if len(running) != 1 or (running[0]["stage"], running[0]["mode"]) not in READ_ONLY_AFTER:
        raise ValueError("seed 来源仍有未收尾写入或控制动作")
    return running[0]["number"]


def resolve_storage(backend: Path, descriptor: dict, source: dict, state: dict, *,
                    live_storage: bool, current_storage=current_storage_binding) -> dict:
    directory = source["directory"]
    later = history(directory, state, descriptor, current=_active_read(state, directory))
    records = [item for item in later if (item["stage"], item["mode"]) == ("seed-runtime", "source-rebind")]
    original = source["storage"]["storage"]
    effective, receipt = original, None
    if records:
        record = records[0]
        receipt = record["result"]
        if bound_file(backend, receipt) != directory / "results" / f"{record['number']:04d}.json":
            raise ValueError("重绑定结果不属于固定 run attempt")
        value = read_json(bound_file(backend, receipt))
        exact(value, FIELDS)
        prefix = [item for item in state["attempts"] if item["number"] < record["number"]]
        if (value["status"] != "seed_source_rebound" or value["source_registration"] != descriptor
                or value["original_storage"] != original or value["history_length"] != len(prefix)
                or value["history_sha256"] != plan_hash(prefix) or value["remote_writes"] != 0
                or value["restore_qualified"] is not False):
            raise ValueError("重绑定没有精确冻结来源及完整历史前缀")
        bound_file(backend, value["review_successor"])
        effective = registered_storage_binding(backend, directory, "target")
        if effective != value["current_storage"]:
            raise ValueError("重绑定后存储代次变化")
        transition(backend, original, effective)
    if live_storage:
        if current_storage(backend, directory, "target") != effective:
            raise ValueError("seed 源 RustFS 已停止、重启或缺少显式重绑定")
        quiet_producers(backend, source)
    result = copy.deepcopy(source)
    result["storage"]["storage"] = copy.deepcopy(effective)
    result["source_rebind"] = receipt
    return result


def register_rebind(backend: Path, directory: Path, request_file: Path, number: int) -> dict:
    from devex_clone_seed_source import _registered_source
    from reference_fixture_successor import _source_with_loader

    request_file = local_path(backend, str(request_file))
    request = binding(request_file)

    def inspect() -> tuple[dict, dict, list]:
        source = _source_with_loader(backend, request, live_storage=False, loader=_registered_source)
        if source["directory"] != directory:
            raise ValueError("source-rebind 请求的 C52 不属于当前 run")
        state = load_state(directory)
        if (not state["attempts"] or state["attempts"][-1]["number"] != number
                or (state["attempts"][-1]["stage"], state["attempts"][-1]["mode"], state["attempts"][-1]["status"])
                != ("seed-runtime", "source-rebind", "running")):
            raise ValueError("source-rebind 必须由同一登记阶段执行")
        later = history(directory, state, source["review_successor"]["source_result"], current=number)
        if any(item["mode"] in {"source-rebind", "arm-input"} and item["number"] != number for item in later):
            raise ValueError("已重绑定或发布 arm 的来源不能再次重绑定")
        quiet_producers(backend, source)
        current = current_storage_binding(backend, directory, "target")
        transition(backend, source["storage"]["storage"], current)
        if binding(request_file) != request:
            raise ValueError("source-rebind 的 successor 关系发生变化")
        return source, current, state["attempts"][:-1]

    before = inspect()
    source, current, prefix = before
    result = {"status": "seed_source_rebound", "source_registration": source["review_successor"]["source_result"],
              "review_successor": request, "original_storage": source["storage"]["storage"],
              "current_storage": current, "history_length": len(prefix), "history_sha256": plan_hash(prefix),
              "remote_writes": 0, "restore_qualified": False}
    if inspect() != before:
        raise ValueError("重绑定期间来源、存储代次或历史变化")
    return result


def published_restart_guard(backend: Path, directory: Path, number: int) -> None:
    from devex_clone_seed_source import _registered_source
    from devex_clone_target_binding import pending_request_binding, request_binding

    state = load_state(directory)
    if (not state["attempts"] or state["attempts"][-1]["number"] != number
            or (state["attempts"][-1]["stage"], state["attempts"][-1]["mode"], state["attempts"][-1]["status"])
            != ("storage-target", "restart", "running")):
        raise ValueError("发布源存储重启不是当前登记阶段")
    records = [item for item in state["attempts"] if item["stage"] == "seed-runtime"
               and item["mode"] == "source-register"]
    if not records or records[-1]["status"] != "passed":
        raise ValueError("seed 存储重启需要完整发布源")
    descriptor = records[-1]["result"]
    later = history(directory, state, descriptor, current=number)
    if any(item["mode"] in {"source-rebind", "arm-input"} for item in later):
        raise ValueError("已交接来源禁止再次重启")

    def validate_history(root: Path, target: dict) -> tuple[dict, dict]:
        review = read_json(bound_file(root, {key: value for key, value in target["review"].items()
                                          if key != "canonical_sha256"}))
        return (request_binding(root, target) if review["ready_for_execution"] else
                pending_request_binding(root, target, target["review"]))

    source = _registered_source(backend, descriptor, live_storage=False, validate_seed_target=validate_history)
    quiet_producers(backend, source)
    original = source["storage"]["storage"]
    directory_identity(backend, original["data_directory"])
    bound_file(backend, original["request"])
    if process_identity(original["storage"]["identity"]["pid"]) is not None:
        raise ValueError("已发布存储旧进程仍存在或 PID 被复用")

"""fresh 目标迁移前缀续作的执行、失败记录与最终发布。"""
from __future__ import annotations

from pathlib import Path
import subprocess
import uuid

from devex_clone import read_json
from devex_clone_capture import write_json
from devex_clone_model import local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import execution_binary_bindings
from devex_clone_target_resume_evidence import migration_resume_state
from devex_clone_target_evidence import evidence
from devex_clone_target_state import generation_lock, now
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_runtime_evidence import fixture_restart_generation
import devex_clone_target_tree_claim as tree_claim


def initialize_resume_state(backend: Path, output: Path, request_descriptor: dict,
                            prepared_files: dict) -> dict:
    """统一判定库存失败或严格迁移前缀，两类证据不能互相降级。"""
    from devex_clone_target_inventory_resume import inventory_resume_state

    inventory = inventory_resume_state(backend, output, request_descriptor, prepared_files)
    if inventory["resumable"]:
        return {**inventory, "mode": "inventory"}
    migration = migration_resume_state(backend, output, request_descriptor,
                                       prepared_files)
    if migration["resumable"]:
        return migration
    return {"resumable": False,
            "reason": f"inventory: {inventory['reason']}；migration: {migration['reason']}"}


def _resource_preimage(resources, request: dict, *, redis_markers: bool) -> dict:
    return {"databases": [resources.database_state(db, exists=True)
                          for db in request["target"]["databases"]],
            "objects": resources.objects(initialized=True),
            "redis": resources.redis_state(initialized=redis_markers,
                                           sentinel=redis_markers)}


def _rebind_restart_redis_markers(output: Path, resources, attempt: str,
                                  resume_intent: Path, started_path: Path,
                                  generation: dict, before: dict,
                                  active: dict, trusted: list[dict]) -> tuple[dict, list[dict]]:
    """为无持久化夹具 Redis 追加一次原子标记恢复及不可重放证据。"""
    intent_path = output / f"resume-redis-markers-{attempt}.intent.json"
    intent = evidence("resume-transition", resources.backend, output,
        transition="marker-intent", arguments={"attempt": attempt, "at": now(),
            "resume": binding(resume_intent), "started": binding(started_path),
            "restart_generation": generation, "request": resources.request, "before": before})
    tree_claim.unchanged(output, trusted)
    write_json(intent_path, intent)
    trusted = tree_claim.claim_one(output, trusted, intent_path, intent)
    active.update({"id": "redis-marker-rebind", "intent": binding(intent_path),
                   "restart_generation": generation, "confirmation": None,
                   "failure": None, "write_started": True, "unknown_result": True})
    try:
        result = resources.rebind_restart_redis_markers()
        after = resources.redis_state(initialized=True, sentinel=True)
    except BaseException as error:
        failure_path = output / f"resume-redis-markers-{attempt}.failure.json"
        try:
            failure = evidence("resume-transition", resources.backend, output,
                transition="marker-failure", arguments={"attempt": attempt, "at": now(),
                    "intent": binding(intent_path), "restart_generation": generation,
                    "error_type": type(error).__name__})
            write_json(failure_path, failure)
            active["failure"] = binding(failure_path)
        except BaseException as diagnostic_error:
            error.add_note(f"Redis 标记重绑失败证据保存失败：{type(diagnostic_error).__name__}")
        raise
    confirmed_path = output / f"resume-redis-markers-{attempt}.confirmed.json"
    confirmed = evidence("resume-transition", resources.backend, output,
        transition="marker-confirmed", arguments={"attempt": attempt, "at": now(),
            "intent": binding(intent_path), "restart_generation": generation,
            "after": after, "atomic_result": result})
    write_json(confirmed_path, confirmed)
    trusted = tree_claim.claim_one(output, trusted, confirmed_path, confirmed)
    active.update({"confirmation": binding(confirmed_path), "unknown_result": False})
    return binding(confirmed_path), trusted


def _operation_receipts(output: Path, operation: dict) -> list[dict]:
    return [binding(path) for path in sorted(output.glob(operation["stage"] + "-*.command.json"))]


def _run_operation(output: Path, resources, operation: dict, index: int,
                    attempt: str, resume_intent: Path, original_failure: dict,
                    active: dict, trusted: list[dict]) -> tuple[dict, list[dict]]:
    operation_intent = output / f"resume-migrate-{attempt}-{index:02d}.intent.json"
    active.update({"id": operation["id"], "index": index, "intent": None,
                   "command_receipts": [], "phase": "operation-intent-checkpoint"})
    body = evidence("resume-transition", resources.backend, output,
                    transition="operation-intent", arguments={"attempt": attempt,
                        "index": index, "at": now(), "resume": binding(resume_intent),
                        "failure": original_failure, "operation": operation})
    tree_claim.unchanged(output, trusted)
    write_json(operation_intent, body)
    trusted = tree_claim.claim_one(output, trusted, operation_intent, body)
    active["intent"] = binding(operation_intent)
    active["phase"] = "pre-command-tree-check"
    tree_claim.unchanged(output, trusted)
    try:
        resources.command(operation["stage"], operation["command"], timeout=1800,
                          diagnostic=active)
    finally:
        active["command_receipts"] = _operation_receipts(output, operation)
    active["phase"] = "command-receipt-verify"
    receipt = evidence("migration-operation-record", resources.backend, output,
                       migration_operation=operation)
    trusted = tree_claim.claim_one(output, trusted, Path(receipt["path"]))
    confirmed_path = output / f"resume-migrate-{attempt}-{index:02d}.confirmed.json"
    confirmation = evidence("resume-transition", resources.backend, output,
                            transition="operation-confirmed", arguments={"attempt": attempt,
                                "index": index, "at": now(), "intent": binding(operation_intent),
                                "receipt": receipt, "write": operation["write"]})
    active["phase"] = "operation-confirmation-publish"
    write_json(confirmed_path, confirmation)
    trusted = tree_claim.claim_one(output, trusted, confirmed_path, confirmation)
    return ({"id": operation["id"], "receipt": receipt,
             "confirmation": binding(confirmed_path)}, trusted)


def _safe_os_error(error: BaseException, active: dict | None, backend: Path, output: Path) -> dict | None:
    """仅提取异常的结构化属性，路径规范化和脱敏由 Rust 处理。"""
    if not isinstance(error, OSError):
        return None
    return evidence("resume-transition", backend, output, transition="os-error",
                    arguments={"errno": error.errno, "winerror": getattr(error, "winerror", None),
                               "filename": getattr(error, "filename", None),
                               "receipt_path": active.get("_receipt_path") if active else None})


def _initialized_result(backend: Path, output: Path, request: dict, original: dict, reset: dict,
                        initial: dict, objects: dict, redis: dict) -> dict:
    return evidence("resume-transition", backend, output, transition="initialized",
                    arguments={"request": request, "generation": original, "reset": reset,
                               "inventory": initial, "objects": objects, "redis": redis,
                               "at": now()})


def resume_migration_target(backend: Path, output: Path, run=subprocess.run, *,
                            storage_run: Path | None = None,
                            request_descriptor: dict | None = None,
                            prepared_files: dict, publish_files=None) -> dict:
    """从首个缺失 verify 起完成迁移、库存和发布，已有命令一条也不重放。"""
    from devex_clone_target import context, inventory, unchanged

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    prepared = read_json(output / "prepare.json")
    expected = prepared["request"] if request_descriptor is None else request_descriptor
    state = migration_resume_state(backend, output, expected, prepared_files)
    if not state["resumable"]:
        raise ValueError("迁移初始化不可续作：" + state["reason"])
    attempt = uuid.uuid4().hex
    intent_path = output / f"resume-initialize-{attempt}.intent.json"
    successor = state["mode"] == "migration-successor"
    session = evidence("resume-transition", backend, output, transition="session-start",
                       arguments={"state": state})
    started_path = output / f"resume-initialize-{attempt}.started.json"
    after_unlock = [] if publish_files is not None else None
    try:
        with generation_lock(output, after_initialize_unlock=after_unlock) as lock_identity:
            if migration_resume_state(backend, output, expected, prepared_files,
                                      owned_lock_identity=lock_identity) != state:
                raise ValueError("取得初始化锁前迁移前缀或固定前像发生变化")
            binaries = execution_binary_bindings(backend, state["request"])
            if successor and binaries != state["protected_binaries"]:
                raise ValueError("迁移后继的受保护二进制不同于首次失败续作")
            resume_intent = evidence("resume-transition", backend, output,
                transition="migration-intent", arguments={"attempt": attempt, "at": now(),
                    "request": expected, "state": state, "protected_binaries": binaries})
            write_json(intent_path, resume_intent)
            trusted = tree_claim.claim_one(output, state["files"], intent_path, resume_intent)
            tree_claim.unchanged(output, trusted)
            current, resources = context(
                backend, state["request"], output, run, storage_run=storage_run,
                owned_lock_identity=lock_identity
            )
            trusted = tree_claim.claim_runtime(
                backend, output, trusted, state["baseline_files"],
                storage_runtime=resources.storage_runtime_binding,
                cache_runtime=resources.cache_runtime_binding
            )
            verify_storage_generation(state["prepared"]["generation"], current,
                                      resources.storage_runtime_binding, resources.cache_runtime_binding)
            tree_claim.unchanged(output, trusted)
            restart_generation = fixture_restart_generation(
                resources.storage_runtime_binding, resources.cache_runtime_binding
            )
            if successor and restart_generation != state["restart_generation"]:
                raise ValueError("迁移后继的夹具服务代次不同于首次失败续作")
            before = _resource_preimage(
                resources, state["request"],
                redis_markers=successor or restart_generation is None
            )
            if successor and before != state["expected_resources"]:
                raise ValueError("迁移后继的资源前像不同于首次失败后的已确认状态")
            started = evidence("resume-transition", backend, output,
                transition="migration-started", arguments={"attempt": attempt, "at": now(),
                    "intent": binding(intent_path), "generation": current,
                    "storage_runtime": resources.storage_runtime_binding,
                    "cache_runtime": resources.cache_runtime_binding, "resources_before": before})
            write_json(started_path, started)
            trusted = tree_claim.claim_preimage(
            backend,                output, trusted, state["baseline_files"], state["request"], started_path, started
            )
            marker_confirmation = state["prior_marker_confirmation"] if successor else None
            if restart_generation is not None and not successor:
                session = evidence("resume-transition", backend, output,
                    transition="session-markers", arguments={"session": session})
                marker_confirmation, trusted = _rebind_restart_redis_markers(
                    output, resources, attempt, intent_path, started_path,
                    restart_generation, before["redis"], session["active_operation"], trusted
                )
                session = evidence("resume-transition", backend, output,
                    transition="session-markers-completed",
                    arguments={"session": session, "confirmation": marker_confirmation})
                unchanged(backend, state["request"], current, resources, run)
                trusted = tree_claim.claim_runtime(
                    backend, output, trusted, state["baseline_files"],
                    storage_runtime=resources.storage_runtime_binding,
                    cache_runtime=resources.cache_runtime_binding
                )
            for index, operation in enumerate(state["operations"][state["next_index"]:],
                                              state["next_index"]):
                session = evidence("resume-transition", backend, output,
                    transition="session-operation", arguments={"session": session,
                        "operation": operation, "index": index})
                completed_item, trusted = _run_operation(
                    output, resources, operation, index, attempt, intent_path,
                    state["failure"], session["active_operation"], trusted
                )
                session = evidence("resume-transition", backend, output,
                    transition="session-completed", arguments={"session": session,
                        "operation": operation, "completed": completed_item})
                unchanged(backend, state["request"], current, resources, run)
                trusted = tree_claim.claim_runtime(
                    backend, output, trusted, state["baseline_files"],
                    storage_runtime=resources.storage_runtime_binding,
                    cache_runtime=resources.cache_runtime_binding
                )
            evidence("migration-prefix", backend, output, operations=state["operations"], complete=True)
            inventory_directory = output / f"inventory-resume-{attempt}"
            tree_claim.unchanged(output, trusted)
            initial, inventory_files = inventory(
                backend, state["request"], resources, inventory_directory
            )
            trusted = tree_claim.claim_inventory(
                output, trusted, inventory_directory, initial, inventory_files
            )
            tree_claim.unchanged(output, trusted)
            objects = resources.objects(initialized=True)
            trusted = tree_claim.claim_objects(
                output, trusted, state["baseline_files"], state["request"]
            )
            tree_claim.unchanged(output, trusted)
            redis = resources.redis_state(initialized=True, sentinel=True)
            tree_claim.unchanged(output, trusted)
            unchanged(backend, state["request"], current, resources, run)
            trusted = tree_claim.claim_runtime(
                backend, output, trusted, state["baseline_files"],
                storage_runtime=resources.storage_runtime_binding,
                cache_runtime=resources.cache_runtime_binding
            )
            bound_file(backend, expected)
            result = _initialized_result(backend, output, state["request"], state["prepared"]["generation"],
                                         state["reset"]["completed"], initial, objects, redis)
            tree_claim.unchanged(output, trusted)
            candidate_path = output / "initialized-candidate.json"
            write_json(candidate_path, result)
            trusted = tree_claim.claim_one(output, trusted, candidate_path, result)
            initialized_path = output / "initialized.json"
            write_json(initialized_path, result)
            trusted = tree_claim.claim_one(output, trusted, initialized_path, result)
            confirmation_path = output / f"resume-initialize-{attempt}.confirmed.json"
            confirmation = evidence("resume-transition", backend, output,
                transition="migration-confirmed", arguments={"attempt": attempt, "at": now(),
                    "intent": binding(intent_path), "started": binding(started_path),
                    "failure": state["failure"], "initialized": binding(initialized_path),
                    "prepared_files": state["prepared_files"], "original_prefix": state["completed"],
                    "resumed_operations": session["completed"], "redis_marker_rebind": marker_confirmation,
                    "remote_write_operations": session["confirmed_writes"],
                    "predecessor": state["predecessor"] if successor else None})
            write_json(confirmation_path, confirmation)
            trusted = tree_claim.claim_one(
                output, trusted, confirmation_path, confirmation
            )
            tree_claim.unchanged(output, trusted)
            if after_unlock is not None:
                after_unlock.append(lambda: publish_files(trusted))
        return result
    except BaseException as error:
        if intent_path.exists():
            failure_path = output / f"resume-initialize-{attempt}.failure.json"
            try:
                os_error = _safe_os_error(error, session["active_operation"], backend, output)
                failure = evidence("resume-transition", backend, output,
                    transition="migration-failure", arguments={
                        "attempt": attempt, "at": now(), "intent": binding(intent_path),
                        "source_failure": state["failure"], "prepared_files": state["prepared_files"],
                        "started": binding(started_path) if started_path.is_file() else None,
                        "error_type": type(error).__name__, "confirmed_operations": session["completed"],
                        "remote_write_operations_confirmed": session["confirmed_writes"],
                        "active_write": session["active_write"], "active_operation": session["active_operation"],
                        "os_error": os_error,
                        "predecessor": state["predecessor"] if successor else None})
                write_json(failure_path, failure)
            except BaseException as diagnostic_error:
                error.add_note(f"迁移续作失败证据保存失败：{type(diagnostic_error).__name__}")
        raise


def resume_initialize_target(backend: Path, output: Path, run=subprocess.run, *,
                             storage_run: Path | None = None,
                             request_descriptor: dict | None = None,
                             prepared_files: dict, publish_files=None) -> dict:
    """按严格判定选择唯一合法的初始化续作方式。"""
    from devex_clone_target_inventory_resume import resume_inventory_target

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    prepared = read_json(output / "prepare.json")
    expected = prepared["request"] if request_descriptor is None else request_descriptor
    state = initialize_resume_state(backend, output, expected, prepared_files)
    if not state["resumable"]:
        raise ValueError("fresh 目标 initialize 不能续作：" + state["reason"])
    if state["mode"] == "inventory":
        return resume_inventory_target(backend, output, run, storage_run=storage_run,
                                       request_descriptor=expected, prepared_files=prepared_files,
                                       publish_files=publish_files)
    return resume_migration_target(backend, output, run, storage_run=storage_run,
                                   request_descriptor=expected, prepared_files=prepared_files,
                                   publish_files=publish_files)

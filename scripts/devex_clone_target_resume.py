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
from devex_clone_target_resume_evidence import (migration_prefix, migration_resume_state,
                                                 _operation_record)
from devex_clone_target_state import generation_lock, now
from devex_clone_target_storage import verify_storage_generation
import devex_clone_target_tree_claim as tree_claim
from restore_build import file_digest
from restore_reference_plan import plan_hash


def initialize_resume_state(backend: Path, output: Path, request_descriptor: dict,
                            prepared_files: dict) -> dict:
    """统一判定库存失败或严格迁移前缀，两类证据不能互相降级。"""
    from devex_clone_target import target_evidence_hooks
    from devex_clone_target_inventory_resume import inventory_resume_state

    inventory = inventory_resume_state(backend, output, request_descriptor, prepared_files)
    if inventory["resumable"]:
        return {**inventory, "mode": "inventory"}
    migration = migration_resume_state(backend, output, request_descriptor,
                                       target_evidence_hooks(), prepared_files)
    if migration["resumable"]:
        return migration
    return {"resumable": False,
            "reason": f"inventory: {inventory['reason']}；migration: {migration['reason']}"}


def _resource_preimage(resources, request: dict) -> dict:
    return {"databases": [resources.database_state(db, exists=True)
                          for db in request["target"]["databases"]],
            "objects": resources.objects(initialized=True),
            "redis": resources.redis_state(initialized=True, sentinel=True)}


def _operation_receipts(output: Path, operation: dict) -> list[dict]:
    return [binding(path) for path in sorted(output.glob(operation["stage"] + "-*.command.json"))]


def _run_operation(output: Path, resources, operation: dict, index: int,
                    attempt: str, resume_intent: Path, original_failure: dict,
                    active: dict, trusted: list[dict]) -> tuple[dict, list[dict]]:
    operation_intent = output / f"resume-migrate-{attempt}-{index:02d}.intent.json"
    active.update({"id": operation["id"], "index": index, "intent": None,
                   "command_receipts": []})
    body = {"format_version": 1, "kind": "devex-clone-migration-operation-intent",
            "attempt": attempt, "index": index, "at": now(), "resume": binding(resume_intent),
            "failure": original_failure, "operation": {key: operation[key]
                for key in ("id", "command", "write")}}
    tree_claim.unchanged(output, trusted)
    write_json(operation_intent, body)
    trusted = tree_claim.claim_one(output, trusted, operation_intent, body)
    active["intent"] = binding(operation_intent)
    tree_claim.unchanged(output, trusted)
    try:
        resources.command(operation["stage"], operation["command"], timeout=1800)
    finally:
        active["command_receipts"] = _operation_receipts(output, operation)
    receipt = _operation_record(output, operation)
    trusted = tree_claim.claim_one(output, trusted, Path(receipt["path"]))
    confirmed_path = output / f"resume-migrate-{attempt}-{index:02d}.confirmed.json"
    confirmation = {
        "format_version": 1, "kind": "devex-clone-migration-operation-confirmed",
        "attempt": attempt, "index": index, "at": now(), "intent": binding(operation_intent),
        "receipt": receipt, "write": operation["write"],
    }
    write_json(confirmed_path, confirmation)
    trusted = tree_claim.claim_one(output, trusted, confirmed_path, confirmation)
    return ({"id": operation["id"], "receipt": receipt,
             "confirmation": binding(confirmed_path)}, trusted)


def _initialized_result(output: Path, request: dict, original: dict, reset: dict,
                        initial: dict, objects: dict, redis: dict) -> dict:
    from devex_clone_target import history

    return {"format_version": 1, "status": "fresh_target_initialized", "id": request["id"],
            "scope_id": request["target"]["scope_id"], "completed_at": now(), "generation": original,
            "prepare_sha256": file_digest(output / "prepare.json")["sha256"], "reset": reset,
            "inventory": initial, "objects": objects, "redis": redis, "history": history(output),
            "controlled_generation_never_started": True, "external_writers_discovered": False,
            "target_ready": False, "clone_verified": False, "restore_qualified": False}


def resume_migration_target(backend: Path, output: Path, run=subprocess.run, *,
                            storage_run: Path | None = None,
                            request_descriptor: dict | None = None,
                            prepared_files: dict, publish_files=None) -> dict:
    """从首个缺失 verify 起完成迁移、库存和发布，已有命令一条也不重放。"""
    from devex_clone_target import context, inventory, target_evidence_hooks, unchanged

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    prepared = read_json(output / "prepare.json")
    expected = prepared["request"] if request_descriptor is None else request_descriptor
    hooks = target_evidence_hooks()
    state = migration_resume_state(backend, output, expected, hooks, prepared_files)
    if not state["resumable"]:
        raise ValueError("迁移初始化不可续作：" + state["reason"])
    attempt = uuid.uuid4().hex
    intent_path = output / f"resume-initialize-{attempt}.intent.json"
    completed, confirmed_writes, active_write, active_operation = [], 0, None, None
    started_path = output / f"resume-initialize-{attempt}.started.json"
    after_unlock = [] if publish_files is not None else None
    try:
        with generation_lock(output, after_initialize_unlock=after_unlock) as lock_identity:
            if migration_resume_state(backend, output, expected, hooks, prepared_files,
                                      owned_lock_identity=lock_identity) != state:
                raise ValueError("取得初始化锁前迁移前缀或固定前像发生变化")
            binaries = execution_binary_bindings(backend, state["request"])
            next_operation = state["operations"][state["next_index"]]
            resume_intent = {
                "format_version": 1, "kind": "devex-clone-migration-resume-intent",
                "attempt": attempt, "at": now(), "request": expected,
                "failure": state["failure"], "prepare": binding(output / "prepare.json"),
                "initialize_started": state["initialize_started"], "reset": state["reset"],
                "creation": state["creation"], "migration_prefix": state["completed"],
                "prepared_files": state["prepared_files"],
                "next_operation": next_operation["id"],
                "protected_binaries": binaries, "remote_write_operations_before_resume": 0,
            }
            write_json(intent_path, resume_intent)
            trusted = tree_claim.claim_one(output, state["files"], intent_path, resume_intent)
            tree_claim.unchanged(output, trusted)
            current, resources = context(
                backend, state["request"], output, run, storage_run=storage_run,
                owned_lock_identity=lock_identity
            )
            trusted = tree_claim.claim_runtime(output, trusted, state["baseline_files"])
            verify_storage_generation(state["prepared"]["generation"], current,
                                      resources.storage_runtime_binding, resources.cache_runtime_binding)
            tree_claim.unchanged(output, trusted)
            before = _resource_preimage(resources, state["request"])
            started = {
                "format_version": 1, "kind": "devex-clone-migration-resume-started",
                "attempt": attempt, "at": now(), "intent": binding(intent_path),
                "current_generation_sha256": plan_hash(current), "storage": current["storage"],
                "storage_runtime": resources.storage_runtime_binding,
                "cache_runtime": resources.cache_runtime_binding, "resources_before": before,
            }
            write_json(started_path, started)
            trusted = tree_claim.claim_preimage(
                output, trusted, state["baseline_files"], state["request"], started_path, started
            )
            for index, operation in enumerate(state["operations"][state["next_index"]:],
                                              state["next_index"]):
                active_write = operation["id"] if operation["write"] else None
                active_operation = {}
                completed_item, trusted = _run_operation(
                    output, resources, operation, index, attempt, intent_path,
                    state["failure"], active_operation, trusted
                )
                completed.append(completed_item)
                confirmed_writes += int(operation["write"])
                active_write, active_operation = None, None
                unchanged(backend, state["request"], current, resources, run)
                trusted = tree_claim.claim_runtime(
                    output, trusted, state["baseline_files"]
                )
            migration_prefix(output, state["operations"], complete=True)
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
            trusted = tree_claim.claim_runtime(output, trusted, state["baseline_files"])
            bound_file(backend, expected)
            result = _initialized_result(output, state["request"], state["prepared"]["generation"],
                                         state["reset"]["completed"], initial, objects, redis)
            tree_claim.unchanged(output, trusted)
            candidate_path = output / "initialized-candidate.json"
            write_json(candidate_path, result)
            trusted = tree_claim.claim_one(output, trusted, candidate_path, result)
            initialized_path = output / "initialized.json"
            write_json(initialized_path, result)
            trusted = tree_claim.claim_one(output, trusted, initialized_path, result)
            confirmation_path = output / f"resume-initialize-{attempt}.confirmed.json"
            confirmation = {
                "format_version": 1, "kind": "devex-clone-migration-resume-confirmed",
                "attempt": attempt, "at": now(), "intent": binding(intent_path),
                "started": binding(started_path), "failure": state["failure"],
                "initialized": binding(initialized_path),
                "prepared_files": state["prepared_files"],
                "original_prefix": state["completed"], "resumed_operations": completed,
                "remote_write_operations": confirmed_writes, "restore_qualified": False,
            }
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
                write_json(failure_path, {
                    "format_version": 1, "kind": "devex-clone-migration-resume-failure",
                    "attempt": attempt, "at": now(), "intent": binding(intent_path),
                    "source_failure": state["failure"], "prepared_files": state["prepared_files"],
                    "started": binding(started_path) if started_path.is_file() else None,
                    "error_type": type(error).__name__, "confirmed_operations": completed,
                    "remote_write_operations_confirmed": confirmed_writes,
                    "unknown_write_operation": active_write,
                    "active_operation": active_operation,
                })
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

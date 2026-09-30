"""fresh 目标库存失败的只读续作判定、执行与最终确认。"""
from pathlib import Path
import re
import subprocess
import uuid

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_export import cli_executable
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_state import generation_lock, now
from restore_build import file_digest
from restore_reference_plan import plan_hash
from devex_clone_target_reset_evidence import validate_plan_receipt
from devex_clone_target_time import ordered, parse_timestamp, timestamp as _timestamp
import devex_clone_target_tree_claim as tree_claim


def _inventory_resume_confirmed(backend: Path, output: Path, initialized: dict) -> bool:
    """仅接受绑定原 inventory 失败和最终初始化收据的显式恢复确认。"""
    confirmations = list(output.glob("resume-initialize-*.confirmed.json"))
    if (len(confirmations) != 1 or any(output.glob("resume-initialize-*.started.json"))
            or any(output.glob("resume-migrate-*.json"))):
        return False
    prepared = read_json(output / "prepare.json")
    from devex_clone_target import initialization_failure_path

    failure_path = initialization_failure_path(backend, output, prepared["request"])
    failure_digest = binding(failure_path)
    path = confirmations[0]
    value = read_json(path)
    exact(value, {"format_version", "kind", "attempt", "at", "intent", "failure",
                  "prepared_files", "initialized", "remote_writes"})
    attempt = value["attempt"]
    intent_path = output / f"resume-initialize-{attempt}.intent.json"
    intent = read_json(intent_path)
    exact(intent, {"format_version", "kind", "attempt", "at", "failure",
                   "prepared_files", "remote_writes"})
    failed = read_json(failure_path)
    from devex_clone_target import FAILURE_FIELDS
    from devex_clone_target_resume_evidence import INITIALIZED_FIELDS

    exact(failed, FAILURE_FIELDS)
    exact(initialized, INITIALIZED_FIELDS)
    stored_initialized = read_json(output / "initialized.json")
    valid_failure = (failed["status"] == "needs_reconciliation"
                     and _timestamp(failed["at"])
                     and failed["stage"] == "inventory"
                     and failed["error_type"] == "InventoryCaptureError"
                     and failed["automatic_retry"] is False
                     and failed["automatic_resource_cleanup"] is False
                     and failed["fresh_target_initialized"] is False
                     and failed["clone_verified"] is False
                     and failed["restore_qualified"] is False)
    valid_intent = (intent["format_version"] == 1
                    and intent["kind"] == "devex-clone-initialize-resume-intent"
                    and intent["attempt"] == attempt and _timestamp(intent["at"])
                    and intent["failure"] == failure_digest
                    and intent["prepared_files"] == value["prepared_files"]
                    and intent["remote_writes"] == 0)
    valid = (value["format_version"] == 1
            and value["kind"] == "devex-clone-initialize-resume-confirmed"
            and _timestamp(value["at"])
            and isinstance(attempt, str) and re.fullmatch(r"[a-f0-9]{32}", attempt) is not None
            and path == output / f"resume-initialize-{attempt}.confirmed.json"
            and value["intent"] == binding(intent_path)
            and value["failure"] == failure_digest
            and value["initialized"] == binding(output / "initialized.json")
            and value["remote_writes"] == 0
            and valid_failure and valid_intent and stored_initialized == initialized
            and _timestamp(initialized["completed_at"])
            and ordered(failed["at"], intent["at"], initialized["completed_at"], value["at"]))
    if valid:
        from devex_clone_target import PREPARE_FIELDS
        from devex_clone_target_binding import prepared_target_files

        exact(prepared, PREPARE_FIELDS)
        prepared_target_files(backend, output, value["prepared_files"], prepared["request"])
        request = read_bound_json(
            local_path(backend, prepared["request"]["path"]), prepared["request"]
        )
        from devex_clone_target_binding import target_files

        plan = read_json(output / "reset-plan.json")
        executable = cli_executable(prepared["generation"]["maintenance"], "reset")
        _inventory_resume_records(output, failure_digest, failed["at"], value["prepared_files"],
                                   plan, executable, request,
                                   {item["path"] for item in target_files(output)},
                                   success_attempt=attempt, success_terminal_at=value["at"])
    return valid


def inventory_resume_confirmed(backend: Path, output: Path, initialized: dict) -> bool:
    """畸形或漂移的库存确认稳定失败关闭。"""
    try:
        return _inventory_resume_confirmed(backend, output, initialized)
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return False


def _retry_plan_evidence(output: Path, attempt: str, plan: dict, executable: str) -> set[str]:
    plan_path = output / f"resume-reset-plan-{attempt}.json"
    if read_json(plan_path) != plan:
        raise ValueError("库存续作 reset plan 不属于原固定计划")
    pattern = re.compile(rf"resume-reset-plan-{attempt}-[a-f0-9]{{32}}\.command\.json")
    commands = [path for path in output.glob(f"resume-reset-plan-{attempt}-*.command.json")
                if pattern.fullmatch(path.name)]
    if len(commands) != 1:
        raise ValueError("库存续作必须保留唯一 reset plan 命令收据")
    validate_plan_receipt(commands[0], executable, plan)
    return {plan_path.name, commands[0].name}


def _inventory_resume_records(output: Path, failure_digest: dict, source_failure_at: str,
                               prepared_files: dict,
                               plan: dict, executable: str, request: dict,
                               available: set[str], *,
                               success_attempt: str | None = None,
                               success_terminal_at: str | None = None) -> dict:
    """只接受完整的只读 inventory 失败尝试，或当前唯一成功尝试。"""
    intents, failures, confirmations = {}, {}, set()
    pattern = re.compile(r"resume-initialize-([a-f0-9]{32})\.(intent|failure|confirmed)\.json")
    for path in sorted(output.glob("resume-initialize-*.json")):
        match = pattern.fullmatch(path.name)
        if match is None:
            raise ValueError("库存续作包含未知或无法归属的阶段证据")
        attempt, kind = match.groups()
        if kind == "confirmed":
            confirmations.add(attempt)
            continue
        value = read_json(path)
        if kind == "intent":
            exact(value, {"format_version", "kind", "attempt", "at", "failure",
                          "prepared_files", "remote_writes"})
            if (value["format_version"] != 1
                    or value["kind"] != "devex-clone-initialize-resume-intent"
                    or value["attempt"] != attempt or not _timestamp(value["at"])
                    or value["failure"] != failure_digest
                    or value["prepared_files"] != prepared_files
                    or value["remote_writes"] != 0):
                raise ValueError("库存续作 intent 没有绑定原只读失败")
            intents[attempt] = {"binding": binding(path), "at": value["at"]}
        else:
            exact(value, {"format_version", "kind", "attempt", "at", "intent", "source_failure",
                          "prepared_files", "error_type", "remote_writes"})
            if (value["format_version"] != 1
                    or value["kind"] != "devex-clone-initialize-resume-failure"
                    or value["attempt"] != attempt or not _timestamp(value["at"])
                    or value["source_failure"] != failure_digest
                    or value["prepared_files"] != prepared_files
                    or not isinstance(value["error_type"], str)
                    or not value["error_type"] or value["remote_writes"] != 0):
                raise ValueError("库存续作失败收据不是只读失败")
            failures[attempt] = value
    expected_success = set() if success_attempt is None else {success_attempt}
    if confirmations != expected_success or set(intents) != set(failures) | expected_success:
        raise ValueError("库存续作包含挂起、重复或未知结果")
    if any(value["intent"] != intents[attempt]["binding"]
           or not ordered(source_failure_at, intents[attempt]["at"], value["at"])
           for attempt, value in failures.items()):
        raise ValueError("库存续作失败收据不属于原 intent")
    intervals = sorted((parse_timestamp(intents[attempt]["at"]),
                        parse_timestamp(value["at"]), attempt)
                       for attempt, value in failures.items())
    if any(before[1] > after[0] for before, after in zip(intervals, intervals[1:])):
        raise ValueError("库存续作失败尝试时间谱系重叠")
    if success_attempt is not None:
        if success_terminal_at is None or not ordered(
                source_failure_at, intents[success_attempt]["at"], success_terminal_at):
            raise ValueError("库存续作成功尝试时间谱系无效")
        if intervals and intervals[-1][1] > parse_timestamp(intents[success_attempt]["at"]):
            raise ValueError("库存续作成功尝试早于既有失败尝试")
    files = {f"resume-initialize-{attempt}.{kind}.json"
             for attempt in intents for kind in ("intent", "confirmed" if attempt in confirmations else "failure")}
    for attempt in intents:
        files.update(_retry_plan_evidence(output, attempt, plan, executable))
        if attempt in failures:
            from devex_clone_inventory_failure import inventory_failure_files

            files.update(inventory_failure_files(
                output, f"inventory-resume-{attempt}", request, available
            ))
    return {"attempts": len(intents), "files": files}


def inventory_resume_state(backend: Path, output: Path, request_descriptor: dict,
                           prepared_files: dict, *,
                           owned_lock_identity: int | None = None) -> dict:
    """只读判定已完成 reset 的 inventory 失败能否显式继续。"""
    from devex_clone_target_binding import (prepared_target_files, target_files,
                                             validate_initialization_delta)
    from devex_clone_target import (FAILURE_FIELDS, PREPARE_FIELDS,
                                    initialization_failure_path, target_evidence_hooks)
    from devex_clone_target_resume_evidence import (_creation_evidence, _initialization_paths,
                                                    _reset_evidence, migration_operations,
                                                    migration_prefix)

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    files_before = target_files(output, ignored={"initialize.lock"},
                                locked_guard=owned_lock_identity is not None)
    prepared = read_json(output / "prepare.json")
    failure_path = initialization_failure_path(backend, output, request_descriptor)
    lock = output / "initialize.lock"
    lock_invalid = (lock.exists() if owned_lock_identity is None else
                    not lock.is_dir() or lock.stat().st_ino != owned_lock_identity)
    if ((output / "initialized.json").exists() or (output / "initialized-candidate.json").exists()
            or lock_invalid
            or any(output.glob("resume-initialize-*.confirmed.json"))
            or any(output.glob("resume-initialize-*.started.json"))
            or any(output.glob("resume-migrate-*.json"))
            or not (output / "initialize.started.json").is_file() or not failure_path.is_file()):
        return {"resumable": False, "reason": "缺少唯一的已释放 inventory 失败现场"}
    failed = read_json(failure_path)
    exact(failed, FAILURE_FIELDS)
    if (failed["status"] != "needs_reconciliation" or not _timestamp(failed["at"])
            or failed["stage"] != "inventory"
            or failed["error_type"] != "InventoryCaptureError"
            or failed["automatic_retry"] is not False
            or failed["automatic_resource_cleanup"] is not False
            or failed["fresh_target_initialized"] is not False
            or failed["clone_verified"] is not False
            or failed["restore_qualified"] is not False):
        return {"resumable": False, "reason": "失败不是可继续的 inventory 采集失败"}
    exact(prepared, PREPARE_FIELDS)
    request = read_bound_json(local_path(backend, request_descriptor["path"]), request_descriptor)
    if (prepared["status"] != "fresh_creation_prepared" or prepared["request"] != request_descriptor
            or prepared["request_sha256"] != plan_hash(request)):
        return {"resumable": False, "reason": "prepare 请求不属于固定 registration"}
    if request != read_json(output / "request.json"):
        return {"resumable": False, "reason": "固定请求文件已变化"}
    baseline = prepared_target_files(backend, output, prepared_files, request_descriptor)
    hooks = target_evidence_hooks()
    creation = _creation_evidence(output, request)
    reset = _reset_evidence(hooks, output, request, prepared["generation"])
    retries = _inventory_resume_records(
        output, binding(failure_path), failed["at"], prepared_files,
        read_json(output / "reset-plan.json"),
        cli_executable(prepared["generation"]["maintenance"], "reset"), request,
        {item["path"] for item in files_before})
    operations = migration_operations(prepared["generation"]["maintenance"])
    completed = migration_prefix(output, operations, complete=True)["completed"]
    required = _initialization_paths(output, creation, reset, completed)
    required.add(failure_path.name)
    required.update(retries["files"])
    if (output / "inventory-initial").is_dir():
        required.add("inventory-initial")
    validate_initialization_delta(output, files_before, baseline, request, required,
                                  observation_rounds=11 + len(completed) + retries["attempts"],
                                  inventory_failed=True)
    if target_files(output, ignored={"initialize.lock"},
                    locked_guard=owned_lock_identity is not None) != files_before:
        raise ValueError("库存续作判定期间目标证据树发生变化")
    return {"resumable": True, "reason": None, "failure": binding(failure_path),
            "prepared_files": prepared_files,
            "baseline_files": baseline, "files": files_before}


def resume_inventory_target(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None = None,
                            request_descriptor: dict | None = None,
                            prepared_files: dict, publish_files=None) -> dict:
    """只恢复 reset 已完成后的库存采集与发布，绝不重放任何资源写入。"""
    from devex_clone_target import (PREPARE_FIELDS, context, history, inventory, reset_plan,
                                    unchanged)
    from devex_clone_target_reset_evidence import reset_completed

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    prepared = read_json(output / "prepare.json")
    exact(prepared, PREPARE_FIELDS)
    expected = prepared["request"] if request_descriptor is None else request_descriptor
    state = inventory_resume_state(backend, output, expected, prepared_files)
    if not state["resumable"]:
        raise ValueError("库存恢复不可继续：" + state["reason"])
    failure_path = bound_file(backend, state["failure"])
    if failure_path.parent != output:
        raise ValueError("库存恢复失败证据不属于当前 fresh 目标")
    attempt = uuid.uuid4().hex
    resume_intent = output / f"resume-initialize-{attempt}.intent.json"
    after_unlock = [] if publish_files is not None else None
    try:
        with generation_lock(output, after_initialize_unlock=after_unlock) as lock_identity:
            locked_state = inventory_resume_state(
                backend, output, expected, prepared_files,
                owned_lock_identity=lock_identity
            )
            if locked_state != state:
                raise ValueError("取得初始化锁前库存续作证据树发生变化")
            intent = {
                "format_version": 1, "kind": "devex-clone-initialize-resume-intent",
                "attempt": attempt, "at": now(), "failure": binding(failure_path),
                "prepared_files": state["prepared_files"],
                "remote_writes": 0,
            }
            write_json(resume_intent, intent)
            trusted = tree_claim.claim_one(output, state["files"], resume_intent, intent)
            prepared = read_json(output / "prepare.json")
            exact(prepared, PREPARE_FIELDS)
            expected = prepared["request"] if request_descriptor is None else request_descriptor
            if prepared["request"] != expected:
                raise ValueError("库存恢复请求不同于固定 registration")
            request = read_bound_json(local_path(backend, expected["path"]), expected)
            if request != read_json(output / "request.json"):
                raise ValueError("库存恢复请求文件已变化")
            tree_claim.unchanged(output, trusted)
            original, resources = context(
                backend, request, output, run, storage_run=storage_run,
                owned_lock_identity=lock_identity
            )
            trusted = tree_claim.claim_runtime(output, trusted, state["baseline_files"])
            if original != prepared["generation"]:
                raise ValueError("库存恢复前来源、工具或服务代次变化")
            tree_claim.unchanged(output, trusted)
            manifest, sha = reset_plan(resources, original["maintenance"], request, original,
                                       f"resume-reset-plan-{attempt}")
            plan_path = output / f"resume-reset-plan-{attempt}.json"
            command_paths = list(output.glob(
                f"resume-reset-plan-{attempt}-*.command.json"
            ))
            if len(command_paths) != 1:
                raise ValueError("库存续作没有产生唯一 reset plan 命令收据")
            trusted = tree_claim.claim_many(
                output, trusted, [plan_path, command_paths[0]]
            )
            validate_plan_receipt(command_paths[0],
                                  cli_executable(original["maintenance"], "reset"),
                                  {"manifest": manifest, "plan_hash": sha})
            reset = reset_completed(output, manifest, sha)
            if read_json(output / "reset.confirmed.json").get("observed") != reset:
                raise ValueError("库存恢复 reset 完成收据不同于当前受控计划")
            inventory_directory = output / f"inventory-resume-{attempt}"
            tree_claim.unchanged(output, trusted)
            initial, inventory_files = inventory(backend, request, resources, inventory_directory)
            trusted = tree_claim.claim_inventory(
                output, trusted, inventory_directory, initial, inventory_files
            )
            tree_claim.unchanged(output, trusted)
            objects = resources.objects(initialized=True)
            trusted = tree_claim.claim_objects(
                output, trusted, state["baseline_files"], request
            )
            tree_claim.unchanged(output, trusted)
            redis = resources.redis_state(initialized=True, sentinel=True)
            tree_claim.unchanged(output, trusted)
            unchanged(backend, request, original, resources, run)
            trusted = tree_claim.claim_runtime(output, trusted, state["baseline_files"])
            result = {"format_version": 1, "status": "fresh_target_initialized", "id": request["id"],
                      "scope_id": request["target"]["scope_id"], "completed_at": now(), "generation": original,
                      "prepare_sha256": file_digest(output / "prepare.json")["sha256"], "reset": reset,
                      "inventory": initial, "objects": objects, "redis": redis, "history": history(output),
                      "controlled_generation_never_started": True, "external_writers_discovered": False,
                      "target_ready": False, "clone_verified": False, "restore_qualified": False}
            candidate_path = output / "initialized-candidate.json"
            write_json(candidate_path, result)
            trusted = tree_claim.claim_one(output, trusted, candidate_path, result)
            initialized_path = output / "initialized.json"
            write_json(initialized_path, result)
            trusted = tree_claim.claim_one(output, trusted, initialized_path, result)
            confirmation_path = output / f"resume-initialize-{attempt}.confirmed.json"
            confirmation = {
                "format_version": 1, "kind": "devex-clone-initialize-resume-confirmed",
                "attempt": attempt, "at": now(), "intent": binding(resume_intent),
                "failure": binding(failure_path), "initialized": binding(initialized_path),
                "prepared_files": state["prepared_files"], "remote_writes": 0,
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
        if resume_intent.is_file():
            write_json(output / f"resume-initialize-{attempt}.failure.json", {
                "format_version": 1, "kind": "devex-clone-initialize-resume-failure",
                "attempt": attempt, "at": now(), "intent": binding(resume_intent),
                "source_failure": binding(failure_path), "prepared_files": state["prepared_files"],
                "error_type": type(error).__name__, "remote_writes": 0,
            })
        raise

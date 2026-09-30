"""fresh 目标迁移前缀的固定序列与只读证据核验。"""
from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
import hashlib
from pathlib import Path
import re

from devex_clone import read_json
from devex_clone_capture import read_bound_json
from devex_clone_export import cli_executable
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import (KEYS, execution_binary_bindings, prepared_target_files,
                                        request_binding, target_files, validate_initialization_delta,
                                        validate_reset_manifest)
from devex_clone_target_runtime_evidence import (fixture_restart_generation,
                                                  validate_redis_marker_rebind,
                                                  validate_started_generation)
from devex_clone_target_reset_evidence import validate_plan_receipt
from devex_clone_target_time import ordered, timestamp as _timestamp
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash

COMMAND_FIELDS = {"command", "returncode", "error_type", "stdout", "stderr"}
FAILURE_FIELDS = {"status", "stage", "at", "error_type", "reason", "automatic_retry",
                  "automatic_resource_cleanup", "fresh_target_initialized", "clone_verified",
                  "restore_qualified"}
PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}
INITIALIZED_FIELDS = {"format_version", "status", "id", "scope_id", "completed_at", "generation",
                      "prepare_sha256", "reset", "inventory", "objects", "redis", "history",
                      "controlled_generation_never_started", "external_writers_discovered",
                      "target_ready", "clone_verified", "restore_qualified"}


@dataclass(frozen=True)
class TargetEvidenceHooks:
    resume_records: Callable[..., dict]
    prepare_artifact: Callable[[str], bool]
    reset_completed: Callable[..., dict]
    history: Callable[[Path], dict]
    verify_initial_inventory: Callable[..., None]


def migration_operations(maintenance: dict) -> list[dict]:
    """返回首次初始化与续作共同使用的唯一迁移顺序。"""
    executable = cli_executable(maintenance, "migrate")
    scopes = [("control", None), *(("tenant-data", key) for key in sorted(KEYS))]
    result = []
    for scope, target in scopes:
        for action in ("up", "verify"):
            suffix = [scope, action] if target is None else [scope, action, "--target", target]
            name = f"{scope}-{action}" if target is None else f"{scope}-{target}-{action}"
            stage_parts = [scope] if target is None else [scope, "--target", target]
            stdout = f"scope={scope} "
            if target is not None:
                stdout += f"target={target} "
            stdout += "migration=completed schema=verified\n" if action == "up" \
                else "migration_ledger=current schema=verified\n"
            result.append({"id": name, "stage": "migrate-" + "-".join(stage_parts) + "-" + action,
                           "command": [executable, *suffix], "write": action == "up",
                           "stdout": stdout})
    return result


def _validate_command_receipt(path: Path, operation: dict) -> dict:
    value = read_json(path)
    exact(value, COMMAND_FIELDS)
    if (value["command"] != operation["command"] or type(value["returncode"]) is not int
            or value["returncode"] != 0 or value["error_type"] is not None
            or value["stderr"] != "" or not isinstance(value["stdout"], str)
            or value["stdout"].replace("\r\n", "\n") != operation["stdout"]):
        raise ValueError("迁移前缀命令、退出状态或成功输出无效")
    return binding(path)


def _command_receipts(output: Path, operations: list[dict]) -> dict[str, dict]:
    receipts = {}
    patterns = {item["id"]: re.compile(re.escape(item["stage"]) + r"-([a-f0-9]{32})\.command\.json")
                for item in operations}
    for path in sorted(output.glob("migrate-*.command.json")):
        matches = [item for item in operations if patterns[item["id"]].fullmatch(path.name)]
        if len(matches) != 1:
            raise ValueError("迁移前缀包含未知命令收据")
        operation = matches[0]
        if operation["id"] in receipts:
            raise ValueError("迁移前缀包含重复命令收据")
        receipts[operation["id"]] = _validate_command_receipt(path, operation)
    return receipts


def migration_prefix(output: Path, operations: list[dict], *, complete: bool = False) -> dict:
    """核验收据恰好构成固定序列的唯一连续前缀。"""
    receipts = _command_receipts(output, operations)
    prefix = []
    for operation in operations:
        receipt = receipts.get(operation["id"])
        if receipt is None:
            break
        prefix.append({"id": operation["id"], "receipt": receipt})
    if set(receipts) != {item["id"] for item in prefix}:
        raise ValueError("迁移收据越过首个缺失操作，不能证明连续前缀")
    if complete:
        if len(prefix) != len(operations):
            raise ValueError("迁移续作没有完成固定序列")
    elif (not prefix or len(prefix) == len(operations)
          or operations[len(prefix)]["write"]):
        raise ValueError("迁移前缀的首个缺失操作必须是只读 verify")
    return {"completed": prefix, "next_index": len(prefix)}


def _stage_record(output: Path, stage: str, kind: str, expected: dict) -> dict:
    value = read_json(output / f"{stage}.{kind}.json")
    field = "resource" if kind == "intent" else "observed"
    exact(value, {"at", "stage", field})
    if not _timestamp(value["at"]) or value["stage"] != stage \
            or value[field] != expected:
        raise ValueError("fresh 创建阶段 intent 或 confirmed 不属于固定目标")
    return binding(output / f"{stage}.{kind}.json")


def _initial_stage_names(request: dict) -> set[str]:
    stages = [*("create-" + item["key"] for item in request["target"]["databases"]),
              "sentinel", "reset"]
    return {f"{stage}.{kind}.json" for stage in stages for kind in ("intent", "confirmed")}


def _creation_evidence(output: Path, request: dict) -> dict:
    result = {}
    for db in sorted(request["target"]["databases"], key=lambda item: item["key"]):
        stage = "create-" + db["key"]
        resource = {key: db[key] for key in ("key", "server_uuid", "database")}
        observed = {"server_uuid": db["server_uuid"], "database": db["database"],
                    "exists": True, "empty_checked": True}
        result[stage] = {"intent": _stage_record(output, stage, "intent", resource),
                         "confirmed": _stage_record(output, stage, "confirmed", observed)}
    reset = request["reset"]
    sentinel = "sentinel"
    observed = {"keys": [], "owner": None, "sentinel": reset["sentinel_value"]}
    result[sentinel] = {
        "intent": _stage_record(output, sentinel, "intent", {"key": reset["sentinel_key"]}),
        "confirmed": _stage_record(output, sentinel, "confirmed", observed),
    }
    expected = {f"{stage}.{kind}.json" for stage in result for kind in ("intent", "confirmed")}
    actual = ({path.name for path in output.glob("create-*.intent.json")} |
              {path.name for path in output.glob("create-*.confirmed.json")} |
              {path.name for path in output.glob("sentinel*.intent.json")} |
              {path.name for path in output.glob("sentinel*.confirmed.json")})
    if actual != expected:
        raise ValueError("fresh 创建阶段包含缺失、重复命名或未知证据")
    return result


def _prepare_resume_stage_names(hooks: TargetEvidenceHooks, backend: Path, output: Path, request_descriptor: dict) -> set[str]:
    """核验初始化前既有的只读 prepare 续作，并返回其阶段文件名。"""
    records = hooks.resume_records(backend, output, request_descriptor)
    if records["pending"]:
        raise ValueError("fresh 初始化前存在未收尾的 prepare 续作")
    for intent in records["intents"].values():
        value = read_json(bound_file(backend, intent))
        if any(not hooks.prepare_artifact(name) for name in value["files_before"]):
            raise ValueError("prepare 续作前像包含初始化写入证据")
    prepare, request = binding(output / "prepare.json"), binding(output / "request.json")
    for kind, value, _ in records["terminals"].values():
        if kind == "confirmed" and (value["prepare"] != prepare or value["request"] != request):
            raise ValueError("prepare 续作确认不属于当前固定结果")
    return {path.name for path in output.glob("resume-prepare-*.json")
            if path.name.endswith((".intent.json", ".confirmed.json"))}


def _validate_stage_names(output: Path, expected: set[str]) -> None:
    actual = {path.name for pattern in ("*.intent.json", "*.confirmed.json")
              for path in output.glob(pattern)}
    if actual != expected:
        raise ValueError("fresh 初始化包含未登记的 intent 或 confirmed 证据")


def _reset_command_evidence(output: Path, maintenance: dict, plans: tuple[dict, dict],
                            completed: dict) -> dict:
    stages = ("reset-plan", "reset-plan-confirm", "reset-execute")
    found = {}
    all_paths = set(output.glob("reset-*.command.json"))
    executable = cli_executable(maintenance, "reset")
    for stage in stages:
        pattern = re.compile(re.escape(stage) + r"-([a-f0-9]{32})\.command\.json")
        paths = [path for path in all_paths if pattern.fullmatch(path.name)]
        if len(paths) != 1:
            raise ValueError("reset 必须为 plan、复核和 execute 各保留唯一命令收据")
        path = paths[0]
        value = read_json(path)
        exact(value, COMMAND_FIELDS)
        if (type(value["returncode"]) is not int or value["returncode"] != 0
                or value["error_type"] is not None or value["stderr"] != ""
                or not isinstance(value["stdout"], str)):
            raise ValueError("reset 命令收据没有证明成功完成")
        if stage == "reset-execute":
            manifest, sha = plans[0]["manifest"], plans[0]["plan_hash"]
            expected_command = [executable, "execute", "--plan-hash", sha,
                                "--confirm-reset", manifest["confirmation_phrase"]]
            report = output / completed["report"]["file"]
            expected_stdout = (f"reset_status=completed\nreset_completed_at={completed['completed_at']}\n"
                               f"reset_report={report}\n")
            if (value["command"] != expected_command
                    or value["stdout"].replace("\r\n", "\n") != expected_stdout):
                raise ValueError("reset execute 收据与固定计划或完成报告不一致")
        else:
            plan = plans[0 if stage == "reset-plan" else 1]
            found[stage] = validate_plan_receipt(path, executable, plan)
            continue
        found[stage] = binding(path)
    if all_paths != {Path(value["path"]) for value in found.values()}:
        raise ValueError("reset 阶段包含未知命令收据")
    return found


def _reset_evidence(hooks: TargetEvidenceHooks, output: Path, request: dict, original: dict) -> dict:
    reset_evidence = ({path.name for path in output.glob("reset-plan*.json")
                       if not path.name.endswith(".command.json")} |
                      {path.name for path in output.glob("reset*.intent.json")} |
                      {path.name for path in output.glob("reset*.confirmed.json")})
    expected = {"reset-plan.json", "reset-plan-confirm.json", "reset.intent.json",
                "reset.confirmed.json"}
    if reset_evidence != expected:
        raise ValueError("reset 阶段包含缺失、重复命名或未知证据")
    first, second = read_json(output / "reset-plan.json"), read_json(output / "reset-plan-confirm.json")
    for value in (first, second):
        exact(value, {"manifest", "plan_hash"})
    if first != second:
        raise ValueError("原 reset 两次固定计划证据不一致")
    validate_reset_manifest(first["manifest"], request, original)
    completed = hooks.reset_completed(output, first["manifest"], first["plan_hash"])
    intent_binding = _stage_record(output, "reset", "intent", {"plan_hash": first["plan_hash"]})
    confirmed_binding = _stage_record(output, "reset", "confirmed", completed)
    commands = _reset_command_evidence(output, original["maintenance"], (first, second), completed)
    return {"plan": binding(output / "reset-plan.json"),
            "plan_confirm": binding(output / "reset-plan-confirm.json"),
            "intent": intent_binding, "confirmed": confirmed_binding, "commands": commands,
            "completed": completed}


def _initialization_paths(output: Path, creation: dict, reset: dict, prefix: list[dict]) -> set[str]:
    paths = {"copy-target.guard", "failure.json", "initialize.started.json", "reset-state"}
    descriptors = [item for stage in creation.values() for item in stage.values()]
    descriptors += [reset[key] for key in ("plan", "plan_confirm", "intent", "confirmed")]
    descriptors += list(reset["commands"].values()) + [item["receipt"] for item in prefix]
    paths.update(Path(item["path"]).relative_to(output).as_posix() for item in descriptors)
    paths.update(Path(reset["completed"][key]["file"]).as_posix()
                 for key in ("report", "ledger"))
    ledger = Path(reset["completed"]["ledger"]["file"])
    previous = ledger.parent / ("." + ledger.name + ".previous")
    paths.add(previous.as_posix())
    return paths


_RESUME_FAILURE_FIELDS = {
    "format_version", "kind", "attempt", "at", "intent", "source_failure",
    "prepared_files", "started", "error_type", "confirmed_operations",
    "remote_write_operations_confirmed", "unknown_write_operation", "active_operation",
}


def _successful_receipt(path: Path) -> dict:
    value = read_json(path)
    exact(value, COMMAND_FIELDS)
    if (not isinstance(value["command"], list) or not value["command"]
            or value["returncode"] != 0 or type(value["returncode"]) is not int
            or value["error_type"] is not None or not isinstance(value["stdout"], str)
            or value["stderr"] != ""):
        raise ValueError("迁移后继只读观察命令收据无效")
    value["stdout"] = value["stdout"].replace("\r\n", "\n")
    return value


def _prior_resume_observation_names(output: Path, current: list[dict], prepared: list[dict],
                                    request: dict, started: dict) -> set[str]:
    """从首次失败续作聚合证据中扣除两轮重启观察与一次数据库前像。"""
    import devex_clone_target_tree_claim as tree_claim

    prepared_names = {item["path"] for item in prepared}
    additions = {item["path"] for item in current if item["path"] not in prepared_names}
    layout_pattern = re.compile(r"storage-layout-[a-f0-9]{32}\.json")
    layouts = [name for name in additions if layout_pattern.fullmatch(name)
               and tree_claim._fixture_storage_layout(
                   output, prepared, name, started["storage_runtime"])]
    if len(layouts) != 2:
        raise ValueError("迁移后继缺少首次续作的两轮存储代次观察")

    redis_pattern = re.compile(r"redis-kernel-[a-f0-9]{32}\.command\.json")
    baseline = [_successful_receipt(output / item["path"]) for item in prepared
                if redis_pattern.fullmatch(item["path"])]
    commands = {}
    pid = str(started["cache_runtime"]["redis"]["pid"])
    for item in baseline:
        command = list(item["command"])
        match = re.fullmatch(r"/proc/(\d+)/(stat|exe)", command[-1])
        if match:
            command[-1] = f"/proc/{pid}/{match.group(2)}"
        commands[tuple(command)] = None
    if len(baseline) != 10 or len(commands) != 5:
        raise ValueError("迁移后继的 Redis prepare 基线无效")
    candidates = {}
    for name in sorted(item for item in additions if redis_pattern.fullmatch(item)):
        receipt = _successful_receipt(output / name)
        candidates.setdefault(tuple(receipt["command"]), []).append((name, receipt))
    selected = []
    for command in commands:
        matches = candidates.get(command, [])
        if len(matches) < 2:
            raise ValueError("迁移后继缺少首次续作的两轮 Redis 代次观察")
        selected.append(matches[:2])
    for round_index in range(2):
        observed = [matches[round_index][1] for matches in selected]
        if not tree_claim._fixture_redis_observations(
                baseline, observed, started["cache_runtime"]):
            raise ValueError("迁移后继的 Redis 重启观察与固定代次不符")

    mysql_names = set()
    for database in request["target"]["databases"]:
        pattern = re.compile(re.escape("mysql-" + database["key"])
                             + r"-[a-f0-9]{32}\.command\.json")
        baseline_paths = [item["path"] for item in prepared if pattern.fullmatch(item["path"])]
        if len(baseline_paths) != 1:
            raise ValueError("迁移后继的 MySQL prepare 基线无效")
        baseline_receipt = _successful_receipt(output / baseline_paths[0])
        expected_stdout = database["server_uuid"] + "\n" + database["database"] + "\n"
        matches = [name for name in sorted(additions) if pattern.fullmatch(name)
                   and _successful_receipt(output / name)["command"]
                   == baseline_receipt["command"]
                   and _successful_receipt(output / name)["stdout"] == expected_stdout]
        if not matches:
            raise ValueError("迁移后继缺少首次续作的数据库前像观察")
        mysql_names.add(matches[0])
    return {*layouts, *mysql_names,
            *(pair[0] for matches in selected for pair in matches)}


def _failed_readonly_resume_records(backend: Path, output: Path, request: dict,
                                    request_descriptor: dict,
                                    failure: dict, prepared_files: dict,
                                    initialize_started: dict, creation: dict, reset: dict,
                                    operations: list[dict], prefix: list[dict], *,
                                    predecessor: dict | None = None,
                                    finalized: bool = False) -> dict | None:
    """只接受 marker 已确认后首个缺失 verify 收据发布失败的一次前驱。"""
    intent_paths = list(output.glob("resume-initialize-*.intent.json"))
    started_paths = list(output.glob("resume-initialize-*.started.json"))
    failure_paths = list(output.glob("resume-initialize-*.failure.json"))
    confirmed_paths = list(output.glob("resume-initialize-*.confirmed.json"))
    marker_paths = list(output.glob("resume-redis-markers-*.json"))
    operation_paths = list(output.glob("resume-migrate-*.json"))
    if predecessor is None and not any((intent_paths, started_paths, failure_paths,
                                        confirmed_paths, marker_paths, operation_paths)):
        return None
    if len(prefix) >= len(operations) or operations[len(prefix)]["write"]:
        raise ValueError("迁移初始化只允许一个未确认只读操作的首次续作前驱")
    if predecessor is None:
        if (len(intent_paths) != 1 or len(started_paths) != 1 or len(failure_paths) != 1
                or confirmed_paths):
            raise ValueError("迁移初始化只允许一个未确认只读操作的首次续作前驱")
        failure_path = failure_paths[0]
    else:
        failure_path = Path(predecessor["path"])
        if failure_path.parent != output:
            raise ValueError("迁移后继的前驱失败路径无效")
    match = re.fullmatch(r"resume-initialize-([a-f0-9]{32})\.failure\.json",
                         failure_path.name)
    if match is None:
        raise ValueError("迁移后继的前驱 attempt 命名无效")
    attempt = match.group(1)
    intent_path = output / f"resume-initialize-{attempt}.intent.json"
    started_path = output / f"resume-initialize-{attempt}.started.json"
    intent, started = read_json(intent_path), read_json(started_path)
    failed = (read_json(failure_path) if predecessor is None
              else read_bound_json(failure_path, predecessor))
    intent_fields = {"format_version", "kind", "attempt", "at", "request", "failure",
                     "prepare", "initialize_started", "reset", "creation", "migration_prefix",
                     "prepared_files", "next_operation", "protected_binaries",
                     "remote_write_operations_before_resume"}
    started_fields = {"format_version", "kind", "attempt", "at", "intent",
                      "current_generation_sha256", "storage", "storage_runtime",
                      "cache_runtime", "resources_before"}
    exact(intent, intent_fields)
    exact(started, started_fields)
    diagnostic = set(failed) == _RESUME_FAILURE_FIELDS | {"phase", "os_error"}
    if not diagnostic:
        exact(failed, _RESUME_FAILURE_FIELDS)
    active = failed["active_operation"]
    base_active = {"id", "index", "intent", "command_receipts"}
    if not isinstance(active, dict):
        raise ValueError("迁移后继的前驱活动操作缺失")
    if diagnostic:
        exact(active, base_active | {"phase", "receipt_path_length",
                                     "receipt_path_limit_risk"})
        exact(failed["os_error"], {"errno", "winerror", "filename_role", "filename_length"})
    else:
        exact(active, base_active)
    index, operation = len(prefix), operations[len(prefix)]
    receipt_length = len(str(output / (operation["stage"] + "-" + "0" * 32
                                        + ".command.json")))
    legacy_path_failure = (not diagnostic and receipt_length >= 260)
    diagnosed_path_failure = (diagnostic
        and receipt_length >= 260
        and failed["phase"] == active["phase"] == "command-receipt-publish"
        and active["receipt_path_length"] == receipt_length
        and active["receipt_path_limit_risk"] is True
        and failed["os_error"]["filename_role"] == "command_receipt"
        and failed["os_error"]["errno"] == 2
        and type(failed["os_error"]["errno"]) is int
        and (failed["os_error"]["winerror"] is None
             or type(failed["os_error"]["winerror"]) is int
             and failed["os_error"]["winerror"] in (2, 3, 206))
        and failed["os_error"]["filename_length"] == receipt_length)
    operation_intent = output / f"resume-migrate-{attempt}-{index:02d}.intent.json"
    if not operation_intent.is_file():
        raise ValueError("迁移后继的首个缺失只读操作 intent 不存在")
    operation_value = read_json(operation_intent)
    exact(operation_value, {"format_version", "kind", "attempt", "index", "at", "resume",
                            "failure", "operation"})
    validate_started_generation(backend, read_json(output / "prepare.json")["generation"], started)
    marker, marker_files, marker_at = validate_redis_marker_rebind(
        backend, output, request, attempt, intent_path, started_path, started
    )
    _validate_owned_resources(backend, request, started["resources_before"], redis_markers=False)
    expected_files = {intent_path.name, started_path.name, failure_path.name,
                      operation_intent.name, *marker_files}
    actual_files = {path.name for path in (*intent_paths, *started_paths, *failure_paths,
                                           *marker_paths, *operation_paths)}
    if (not (legacy_path_failure or diagnosed_path_failure)
            or intent["format_version"] != 1
            or intent["kind"] != "devex-clone-migration-resume-intent"
            or intent["attempt"] != attempt or intent["request"] != request_descriptor
            or intent["failure"] != failure or intent["prepare"] != binding(output / "prepare.json")
            or intent["initialize_started"] != initialize_started or intent["reset"] != reset
            or intent["creation"] != creation or intent["migration_prefix"] != prefix
            or intent["prepared_files"] != prepared_files
            or intent["next_operation"] != operation["id"]
            or intent["protected_binaries"] != execution_binary_bindings(backend, request)
            or intent["remote_write_operations_before_resume"] != 0
            or started["format_version"] != 1
            or started["kind"] != "devex-clone-migration-resume-started"
            or started["attempt"] != attempt or started["intent"] != binding(intent_path)
            or failed["format_version"] != 1
            or failed["kind"] != "devex-clone-migration-resume-failure"
            or failed["attempt"] != attempt or failed["intent"] != binding(intent_path)
            or failed["source_failure"] != failure or failed["prepared_files"] != prepared_files
            or failed["started"] != binding(started_path)
            or failed["error_type"] != "FileNotFoundError"
            or failed["confirmed_operations"] != []
            or failed["remote_write_operations_confirmed"] != 1
            or failed["unknown_write_operation"] is not None
            or active["id"] != operation["id"] or active["index"] != index
            or active["intent"] != binding(operation_intent)
            or active["command_receipts"] != []
            or operation_value["format_version"] != 1
            or operation_value["kind"] != "devex-clone-migration-operation-intent"
            or operation_value["attempt"] != attempt or operation_value["index"] != index
            or operation_value["resume"] != binding(intent_path)
            or operation_value["failure"] != failure
            or operation_value["operation"] != {key: operation[key]
                for key in ("id", "command", "write")}
            or marker is None or not finalized and actual_files != expected_files
            or not _timestamp(intent["at"]) or not _timestamp(started["at"])
            or not _timestamp(failed["at"]) or not _timestamp(operation_value["at"])
            or not ordered(read_json(output / "failure.json")["at"], intent["at"], started["at"])
            or not ordered(marker_at, operation_value["at"], failed["at"])
            or not finalized and list(output.glob(operation["stage"] + "-*.command.json"))):
        raise ValueError("迁移后继的前驱失败、只读操作或证据闭集无效")
    expected_resources = {"databases": started["resources_before"]["databases"],
                          "objects": started["resources_before"]["objects"],
                          "redis": read_json(Path(marker["path"]))["after"]}
    _validate_owned_resources(backend, request, expected_resources)
    return {"attempt": attempt, "intent": binding(intent_path), "started": binding(started_path),
            "failure": binding(failure_path), "marker_confirmation": marker,
            "restart_generation": fixture_restart_generation(
                started["storage_runtime"], started["cache_runtime"]),
            "protected_binaries": intent["protected_binaries"],
            "expected_resources": expected_resources, "record_files": expected_files,
            "started_value": started, "failure_at": failed["at"]}


def migration_resume_state(backend: Path, output: Path, request_descriptor: dict,
                           hooks: TargetEvidenceHooks, prepared_files: dict, *,
                           owned_lock_identity: int | None = None) -> dict:
    """只读判定 FileNotFoundError 是否停在唯一可恢复的迁移 verify 边界。"""
    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    files_before = target_files(output, ignored={"initialize.lock"},
                                locked_guard=owned_lock_identity is not None)
    from devex_clone_target import initialization_failure_path

    failure_path = initialization_failure_path(backend, output, request_descriptor)
    lock = output / "initialize.lock"
    lock_invalid = (lock.exists() if owned_lock_identity is None else
                    not lock.is_dir() or lock.stat().st_ino != owned_lock_identity)
    if ((output / "initialized.json").exists() or (output / "initialized-candidate.json").exists()
            or lock_invalid or not (output / "initialize.started.json").is_file()
            or not failure_path.is_file()
            or (output / "inventory-initial").exists() or any(output.glob("inventory-resume-*"))):
        return {"resumable": False, "reason": "缺少唯一的迁移中断现场"}
    failed = read_json(failure_path)
    exact(failed, FAILURE_FIELDS)
    if (failed["status"] != "needs_reconciliation" or not _timestamp(failed["at"])
            or failed["stage"] != "create_and_reset"
            or failed["error_type"] != "FileNotFoundError" or failed["automatic_retry"] is not False
            or failed["automatic_resource_cleanup"] is not False
            or failed["fresh_target_initialized"] is not False or failed["clone_verified"] is not False
            or failed["restore_qualified"] is not False):
        return {"resumable": False, "reason": "失败不是可证明前缀的迁移文件访问中断"}
    prepared = read_json(output / "prepare.json")
    exact(prepared, PREPARE_FIELDS)
    request = read_bound_json(local_path(backend, request_descriptor["path"]), request_descriptor)
    baseline = prepared_target_files(backend, output, prepared_files, request_descriptor)
    create_intents = [{"key": db["key"], "server_uuid": db["server_uuid"],
                       "database": db["database"]} for db in request["target"]["databases"]]
    if (prepared["status"] != "fresh_creation_prepared" or prepared["request"] != request_descriptor
            or prepared["request_sha256"] != plan_hash(request) or request != read_json(output / "request.json")
            or prepared["fresh_target_initialized"] is not False
            or prepared["controlled_generation_only"] is not True
            or prepared["external_writers_discovered"] is not False
            or prepared["create_intents"] != create_intents):
        raise ValueError("迁移续作请求或 prepare 不属于固定 registration")
    started = read_json(output / "initialize.started.json")
    exact(started, {"at", "generation_sha256"})
    if (not _timestamp(started["at"])
            or not ordered(started["at"], failed["at"])
            or started["generation_sha256"] != plan_hash(prepared["generation"])):
        raise ValueError("迁移续作的初始化前像摘要无效")
    creation = _creation_evidence(output, request)
    prepare_resume_names = _prepare_resume_stage_names(hooks, backend, output, request_descriptor)
    reset = _reset_evidence(hooks, output, request, prepared["generation"])
    operations = migration_operations(prepared["generation"]["maintenance"])
    prefix = migration_prefix(output, operations)
    prior = _failed_readonly_resume_records(
        backend, output, request, request_descriptor, binding(failure_path), prepared_files,
        binding(output / "initialize.started.json"), creation, reset, operations,
        prefix["completed"]
    )
    prior_stage_names = (set() if prior is None else
                         {name for name in prior["record_files"]
                          if name.endswith((".intent.json", ".confirmed.json"))})
    _validate_stage_names(
        output, _initial_stage_names(request) | prepare_resume_names | prior_stage_names
    )
    required = _initialization_paths(output, creation, reset, prefix["completed"])
    required.add(failure_path.name)
    required.update(prepare_resume_names)
    current_for_initial = files_before
    initialized_objects = None
    if prior is not None:
        required.update(prior["record_files"])
        excluded = _prior_resume_observation_names(
            output, files_before, baseline, request, prior["started_value"]
        )
        current_for_initial = [item for item in files_before if item["path"] not in excluded]
        initialized_objects = prior["started_value"]["resources_before"]["objects"]
    validate_initialization_delta(
        output, current_for_initial, baseline, request,
        required,
        observation_rounds=12 + len(prefix["completed"]),
        initialized_objects=initialized_objects,
    )
    if target_files(output, ignored={"initialize.lock"},
                    locked_guard=owned_lock_identity is not None) != files_before:
        raise ValueError("迁移续作判定期间目标证据树发生变化")
    return {"resumable": True, "reason": None,
            "mode": "migration" if prior is None else "migration-successor",
            "prepared": prepared, "request": request, "failure": binding(failure_path),
            "initialize_started": binding(output / "initialize.started.json"),
            "creation": creation, "reset": reset, "operations": operations,
            "completed": prefix["completed"], "next_index": prefix["next_index"],
            "prepared_files": prepared_files, "baseline_files": baseline,
            "files": files_before, "predecessor": None if prior is None else prior["failure"],
            "prior_marker_confirmation": None if prior is None else prior["marker_confirmation"],
            "restart_generation": None if prior is None else prior["restart_generation"],
            "protected_binaries": None if prior is None else prior["protected_binaries"],
            "expected_resources": None if prior is None else prior["expected_resources"]}


def _validate_owned_resources(backend: Path, request: dict, value: dict, *,
                              redis_markers: bool = True) -> None:
    """验证迁移后的 fresh 资源前像仍只包含本代次 owner。"""
    exact(value, {"databases", "objects", "redis"})
    databases = [{"server_uuid": item["server_uuid"], "database": item["database"],
                  "exists": True, "empty_checked": False}
                 for item in request["target"]["databases"]]
    if value["databases"] != databases or not isinstance(value["objects"], dict) \
            or set(value["objects"]) != set(BUCKETS):
        raise ValueError("迁移续作的数据库或对象前像不属于固定 fresh 目标")
    scope = request["target"]["scope_id"]
    for bucket in sorted(BUCKETS):
        observed = value["objects"][bucket]
        exact(observed, {"keys", "owner"})
        exact(observed["owner"], {"bytes", "sha256"})
        owner = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
        expected_owner = {"bytes": len(owner), "sha256": hashlib.sha256(owner).hexdigest()}
        if (observed["keys"] != [f"{scope}/.ryframe-owner"]
                or observed["owner"] != expected_owner):
            raise ValueError("迁移续作的对象 owner 前像无效")
    _, selected = request_binding(backend, request)
    redis = selected["redis"]
    reset = request["reset"]
    expected_redis = ({"keys": [redis["ownership_key"]], "owner": redis["ownership_value"],
                       "sentinel": reset["sentinel_value"]} if redis_markers else
                      {"keys": [], "owner": None, "sentinel": None})
    if value["redis"] != expected_redis:
        raise ValueError("迁移续作的 Redis owner 前像无效")


def _operation_record(output: Path, operation: dict) -> dict:
    paths = list(output.glob(operation["stage"] + "-*.command.json"))
    pattern = re.compile(re.escape(operation["stage"]) + r"-([a-f0-9]{32})\.command\.json")
    if len(paths) != 1 or pattern.fullmatch(paths[0].name) is None:
        raise ValueError("续作操作没有产生唯一成功命令收据")
    return _validate_command_receipt(paths[0], operation)


def _migration_resume_confirmed(backend: Path, output: Path, initialized: dict,
                                hooks: TargetEvidenceHooks) -> bool:
    all_confirmations = list(output.glob("resume-initialize-*.confirmed.json"))
    confirmations = []
    for path in all_confirmations:
        value = read_json(path)
        if value.get("kind") == "devex-clone-migration-resume-confirmed":
            confirmations.append((path, value))
    resume_failures = list(output.glob("resume-initialize-*.failure.json"))
    if len(confirmations) != 1 or len(all_confirmations) != 1 or len(resume_failures) > 1:
        return False
    path, value = confirmations[0]
    successor = len(resume_failures) == 1
    confirmation_fields = {"format_version", "kind", "attempt", "at", "intent", "started",
                           "failure", "prepared_files", "initialized", "original_prefix",
                           "resumed_operations", "redis_marker_rebind",
                           "remote_write_operations", "restore_qualified"}
    exact(value, confirmation_fields | ({"predecessor"} if successor else set()))
    attempt = value["attempt"]
    if (not isinstance(attempt, str) or re.fullmatch(r"[a-f0-9]{32}", attempt) is None
            or path != output / f"resume-initialize-{attempt}.confirmed.json"
            or not isinstance(value["original_prefix"], list)
            or not isinstance(value["resumed_operations"], list)
            or any(not isinstance(item, dict) for item in value["resumed_operations"])):
        return False
    intent_path = output / f"resume-initialize-{attempt}.intent.json"
    started_path = output / f"resume-initialize-{attempt}.started.json"
    intent_value, started_value = read_json(intent_path), read_json(started_path)
    prepared = read_json(output / "prepare.json")
    from devex_clone_target import initialization_failure_path

    failure_path = initialization_failure_path(backend, output, prepared["request"])
    failed = read_json(failure_path)
    exact(failed, FAILURE_FIELDS)
    exact(initialized, INITIALIZED_FIELDS)
    intent_fields = {"format_version", "kind", "attempt", "at", "request", "failure", "prepare",
                     "prepared_files", "initialize_started", "reset", "creation",
                     "migration_prefix", "next_operation", "protected_binaries",
                     "remote_write_operations_before_resume"}
    exact(intent_value, intent_fields | ({"predecessor"} if successor else set()))
    exact(started_value, {"format_version", "kind", "attempt", "at", "intent",
                          "current_generation_sha256", "storage", "storage_runtime",
                          "cache_runtime", "resources_before"})
    request = read_bound_json(bound_file(backend, intent_value["request"]), intent_value["request"])
    exact(prepared, PREPARE_FIELDS)
    if (initialized["format_version"] != 1 or initialized["status"] != "fresh_target_initialized"
            or initialized["id"] != request["id"]
            or initialized["scope_id"] != request["target"]["scope_id"]
            or initialized["generation"] != prepared["generation"]
            or initialized["prepare_sha256"] != file_digest(output / "prepare.json")["sha256"]
            or initialized["controlled_generation_never_started"] is not True
            or initialized["external_writers_discovered"] is not False
            or initialized["target_ready"] is not False
            or initialized["clone_verified"] is not False
            or initialized["restore_qualified"] is not False
            or read_json(output / "initialized-candidate.json") != initialized
            or initialized["history"] != hooks.history(output)):
        return False
    prepare_resume_names = _prepare_resume_stage_names(hooks, backend, output, prepared["request"])
    creation = _creation_evidence(output, request)
    reset = _reset_evidence(hooks, output, request, initialized["generation"])
    if initialized["reset"] != reset["completed"]:
        return False
    hooks.verify_initial_inventory(backend, output, initialized["inventory"]["receipt"],
                                   resumed=True)
    operations = migration_operations(initialized["generation"]["maintenance"])
    prefix = migration_prefix(output, operations, complete=True)["completed"]
    resumed_ids = [item.get("id") for item in value["resumed_operations"]]
    split = len(value["original_prefix"])
    if not 0 < split < len(operations) or operations[split]["write"]:
        return False
    validate_started_generation(backend, initialized["generation"], started_value)
    prior_files = set()
    predecessor = None
    if successor:
        predecessor = _failed_readonly_resume_records(
            backend, output, request, prepared["request"], binding(failure_path),
            value["prepared_files"], binding(output / "initialize.started.json"),
            creation, reset, operations, prefix[:split],
            predecessor=value["predecessor"], finalized=True
        )
        if predecessor["failure"] != value["predecessor"]:
            return False
        marker_confirmation = predecessor["marker_confirmation"]
        marker_files = {name for name in predecessor["record_files"]
                        if name.startswith("resume-redis-markers-")}
        prior_files = predecessor["record_files"]
        marker_at = started_value["at"]
        _validate_owned_resources(backend, request, started_value["resources_before"])
        if (fixture_restart_generation(started_value["storage_runtime"],
                                       started_value["cache_runtime"])
                != predecessor["restart_generation"]
                or started_value["resources_before"] != predecessor["expected_resources"]):
            return False
    else:
        marker_confirmation, marker_files, marker_at = validate_redis_marker_rebind(
            backend, output, request, attempt, intent_path, started_path, started_value
        )
        _validate_owned_resources(
            backend, request, started_value["resources_before"],
            redis_markers=marker_confirmation is None
        )
    _validate_owned_resources(backend, request, {
        "databases": started_value["resources_before"]["databases"],
        "objects": initialized["objects"],
        "redis": initialized["redis"],
    })
    expected_writes = sum(item["write"] for item in operations[split:]) \
        + int(marker_confirmation is not None)
    expected_resumed = []
    expected_resume_files = {intent_path.name, started_path.name, path.name,
                             *marker_files, *prior_files}
    previous_at = marker_at
    for index, operation in enumerate(operations[split:], split):
        intent = output / f"resume-migrate-{attempt}-{index:02d}.intent.json"
        confirmed = output / f"resume-migrate-{attempt}-{index:02d}.confirmed.json"
        expected_resume_files.update((intent.name, confirmed.name))
        operation_intent, operation_confirmed = read_json(intent), read_json(confirmed)
        exact(operation_intent, {"format_version", "kind", "attempt", "index", "at", "resume",
                                 "failure", "operation"})
        exact(operation_confirmed, {"format_version", "kind", "attempt", "index", "at", "intent",
                                    "receipt", "write"})
        receipt = prefix[index]["receipt"]
        if (operation_intent["format_version"] != 1 or not _timestamp(operation_intent["at"])
                or operation_intent["kind"] != "devex-clone-migration-operation-intent"
                or operation_intent["attempt"] != attempt or operation_intent["index"] != index
                or operation_intent["resume"] != binding(intent_path)
                or operation_intent["failure"] != value["failure"]
                or operation_intent["operation"] != {key: operation[key]
                    for key in ("id", "command", "write")}
                or operation_confirmed["format_version"] != 1
                or not _timestamp(operation_confirmed["at"])
                or operation_confirmed["kind"] != "devex-clone-migration-operation-confirmed"
                or operation_confirmed["attempt"] != attempt or operation_confirmed["index"] != index
                or operation_confirmed["intent"] != binding(intent)
                or operation_confirmed["receipt"] != receipt
                or not ordered(previous_at, operation_intent["at"], operation_confirmed["at"])
                or operation_confirmed["write"] is not operation["write"]):
            return False
        previous_at = operation_confirmed["at"]
        expected_resumed.append({"id": operation["id"], "receipt": receipt,
                                 "confirmation": binding(confirmed)})
    actual_resume_files = ({item.name for item in output.glob("resume-initialize-*.json")} |
                           {item.name for item in output.glob("resume-redis-markers-*.json")} |
                           {item.name for item in output.glob("resume-migrate-*.json")})
    expected_stage_files = (_initial_stage_names(request) | prepare_resume_names |
                            {name for name in expected_resume_files
                             if name.endswith((".intent.json", ".confirmed.json"))})
    actual_stage_files = {path.name for pattern in ("*.intent.json", "*.confirmed.json")
                          for path in output.glob(pattern)}
    if (value["format_version"] != 1 or value["kind"] != "devex-clone-migration-resume-confirmed"
            or not _timestamp(value["at"])
            or not _timestamp(initialized["completed_at"])
            or value["intent"] != binding(intent_path)
            or value["started"] != binding(started_path) or value["failure"] != binding(failure_path)
            or value["initialized"] != binding(output / "initialized.json")
            or value["prepared_files"] != intent_value["prepared_files"]
            or value["original_prefix"] != prefix[:split]
            or value["resumed_operations"] != expected_resumed
            or value["redis_marker_rebind"] != marker_confirmation
            or resumed_ids != [item["id"] for item in operations[split:]]
            or value["remote_write_operations"] != expected_writes
            or value["restore_qualified"] is not False or intent_value["attempt"] != attempt
            or intent_value["format_version"] != 1
            or intent_value["kind"] != "devex-clone-migration-resume-intent"
            or intent_value["request"] != prepared["request"]
            or intent_value["prepared_files"] != value["prepared_files"]
            or request != read_json(output / "request.json")
            or intent_value["failure"] != binding(failure_path)
            or intent_value["prepare"] != binding(output / "prepare.json")
            or intent_value["initialize_started"] != binding(output / "initialize.started.json")
            or intent_value["reset"] != reset
            or intent_value["creation"] != creation
            or intent_value["protected_binaries"] != execution_binary_bindings(backend, request)
            or intent_value["migration_prefix"] != value["original_prefix"]
            or intent_value["next_operation"] != operations[split]["id"]
            or intent_value["remote_write_operations_before_resume"] != int(successor)
            or successor and (intent_value["predecessor"] != value["predecessor"]
                              or value["predecessor"] != binding(resume_failures[0]))
            or started_value["format_version"] != 1
            or started_value["kind"] != "devex-clone-migration-resume-started"
            or not _timestamp(intent_value["at"]) or not _timestamp(started_value["at"])
            or not ordered(predecessor["failure_at"] if successor else failed["at"],
                           intent_value["at"], started_value["at"])
            or not ordered(previous_at, initialized["completed_at"], value["at"])
            or started_value["attempt"] != attempt or started_value["intent"] != binding(intent_path)
            or actual_stage_files != expected_stage_files
            or actual_resume_files != expected_resume_files):
        return False
    prepared_target_files(backend, output, value["prepared_files"], prepared["request"])
    return (failed["status"] == "needs_reconciliation" and _timestamp(failed["at"])
            and failed["stage"] == "create_and_reset"
            and failed["error_type"] == "FileNotFoundError"
            and failed["automatic_retry"] is False and failed["automatic_resource_cleanup"] is False
            and failed["fresh_target_initialized"] is False and failed["clone_verified"] is False
            and failed["restore_qualified"] is False)


def migration_resume_confirmed(backend: Path, output: Path, initialized: dict,
                               hooks: TargetEvidenceHooks) -> bool:
    """畸形或漂移的成功确认只会保持原失败，不会让消费者接受初始化。"""
    try:
        return _migration_resume_confirmed(backend, output, initialized, hooks)
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return False

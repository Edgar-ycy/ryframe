"""关闭固定 seed 运行对：先停 API，排空任务与 outbox，再停 Worker 并终检。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import time
import uuid

from devex_clone_capture import read_json
from process_environment import Environments
from devex_clone_model import exact
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from devex_clone_target_resources import Resources
from full_stack_process import read_process
from restore_reference_plan import plan_hash


DRAIN_TIMEOUT_SECONDS = 15 * 60
DRAIN_STABLE_SECONDS = 2
DRAIN_POLL_SECONDS = 1

ACTIVE_QUERIES = {
    "background_job_status": "SELECT COUNT(*) FROM sys_background_job WHERE status NOT IN ('succeeded','dead')",
    "background_job_lease": "SELECT COUNT(*) FROM sys_background_job WHERE lease_owner IS NOT NULL OR lease_until IS NOT NULL",
    "background_job_completion": "SELECT COUNT(*) FROM sys_background_job WHERE completed_at IS NULL",
    "background_job_attempt": "SELECT COUNT(*) FROM sys_background_job_attempt WHERE "
        "outcome NOT IN ('succeeded','failed','dead','deferred','lease_expired') OR closed_at IS NULL OR NOT "
        "((outcome='lease_expired' AND finished_at IS NULL) OR "
        "(outcome IN ('succeeded','failed','dead','deferred') AND finished_at IS NOT NULL))",
    "outbox_event_status": "SELECT COUNT(*) FROM sys_outbox_event WHERE status NOT IN ('published','dead')",
    "outbox_event_lease": "SELECT COUNT(*) FROM sys_outbox_event WHERE lease_owner IS NOT NULL OR lease_until IS NOT NULL",
    "export_job_status": "SELECT COUNT(*) FROM sys_export_job WHERE status NOT IN ('succeeded','failed','cancelled','expired')",
    "export_job_request": "SELECT COUNT(*) FROM sys_export_job WHERE active_request_fingerprint IS NOT NULL",
    "export_job_delete": "SELECT COUNT(*) FROM sys_export_job WHERE delete_pending_at IS NOT NULL",
    "user_import_job": "SELECT COUNT(*) FROM sys_user_import_job WHERE status NOT IN ('succeeded','partial','failed','cancelled')",
    "enabled_schedule": "SELECT COUNT(*) FROM sys_job_schedule WHERE del_flag<>'2' AND enabled<>0",
    "schedule_execution": "SELECT COUNT(*) FROM sys_job_schedule_execution WHERE "
        "outcome NOT IN ('enqueued','skipped_misfire','skipped_concurrency','target_unavailable','invalid_configuration') "
        "OR (outcome='enqueued' AND background_job_id IS NULL) "
        "OR (outcome<>'enqueued' AND background_job_id IS NOT NULL)",
    "config_bundle": "SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status NOT IN ('succeeded','failed','expired')",
    "config_transfer": "SELECT COUNT(*) FROM sys_tenant_config_transfer WHERE "
        "status NOT IN ('preview_ready','previewed','applied','rolled_back','failed')",
    "retention_run": "SELECT COUNT(*) FROM sys_data_retention_run WHERE status NOT IN ('succeeded','failed')",
    "password_reset": "SELECT COUNT(*) FROM password_reset_requests WHERE status NOT IN ('completed','expired')",
    "tenant_operation_lease": "SELECT COUNT(*) FROM sys_tenant_operation_lease",
    "tenant_data_migration": "SELECT COUNT(*) FROM sys_tenant_data_migration",
    "tenant_data_migration_item": "SELECT COUNT(*) FROM sys_tenant_data_migration_item",
    "file_upload": "SELECT COUNT(*) FROM sys_file WHERE upload_status<>'ready'",
    "tenant_status": "SELECT COUNT(*) FROM sys_tenant WHERE status NOT IN ('enabled','disabled')",
}

TERMINAL_QUERIES = {
    "background_job_dead": "SELECT COUNT(*) FROM sys_background_job WHERE status='dead'",
    "background_job_attempt_failed": "SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='failed'",
    "background_job_attempt_dead": "SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='dead'",
    "background_job_attempt_deferred": "SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='deferred'",
    "background_job_attempt_lease_expired": "SELECT COUNT(*) FROM sys_background_job_attempt WHERE outcome='lease_expired'",
    "outbox_event_dead": "SELECT COUNT(*) FROM sys_outbox_event WHERE status='dead'",
    "export_job_failed": "SELECT COUNT(*) FROM sys_export_job WHERE status='failed'",
    "export_job_cancelled": "SELECT COUNT(*) FROM sys_export_job WHERE status='cancelled'",
    "export_job_expired": "SELECT COUNT(*) FROM sys_export_job WHERE status='expired'",
    "user_import_partial": "SELECT COUNT(*) FROM sys_user_import_job WHERE status='partial'",
    "user_import_failed": "SELECT COUNT(*) FROM sys_user_import_job WHERE status='failed'",
    "user_import_cancelled": "SELECT COUNT(*) FROM sys_user_import_job WHERE status='cancelled'",
    "schedule_skipped_misfire": "SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='skipped_misfire'",
    "schedule_skipped_concurrency": "SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='skipped_concurrency'",
    "schedule_target_unavailable": "SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='target_unavailable'",
    "schedule_invalid_configuration": "SELECT COUNT(*) FROM sys_job_schedule_execution WHERE outcome='invalid_configuration'",
    "config_bundle_failed": "SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status='failed'",
    "config_bundle_expired": "SELECT COUNT(*) FROM sys_tenant_config_bundle WHERE status='expired'",
    "config_transfer_failed": "SELECT COUNT(*) FROM sys_tenant_config_transfer WHERE status='failed'",
    "retention_run_failed": "SELECT COUNT(*) FROM sys_data_retention_run WHERE status='failed'",
    "password_reset_expired": "SELECT COUNT(*) FROM password_reset_requests WHERE status='expired'",
}


def _json_object(fields: dict[str, str]) -> str:
    return "JSON_OBJECT(" + ",".join(f"'{key}',({query})" for key, query in fields.items()) + ")"


DRAIN_SQL = ("SELECT JSON_OBJECT('database_time',"
             "DATE_FORMAT(UTC_TIMESTAMP(6),'%Y-%m-%dT%H:%i:%s.%fZ'),"
             f"'active',{_json_object(ACTIVE_QUERIES)},"
             f"'terminal_non_success',{_json_object(TERMINAL_QUERIES)});")

RESULT_FIELDS = {
    "format_version", "kind", "status", "phase", "handoff", "runtime_directory", "scope_id",
    "process_identities", "runtime_status", "api_stop", "drain", "worker_stop", "final_snapshot", "last_snapshot",
    "terminal_non_success", "stopped_evidence", "worker_preserved", "reason", "error_type",
    "outbox_drained", "direct_remote_writes", "worker_drain_writes", "restore_qualified",
}
REASONS = {
    "api_stop_failed", "post_api_stop_observation_failed", "drain_timeout", "drain_observation_failed",
    "guard_failed", "worker_stop_failed",
    "worker_stop_result_unavailable", "final_verification_failed", "final_state_not_drained",
}
MISSING_RESULT = object()
CHECKPOINT_FILES = {
    "running": "close-10-running.json",
    "api_stopped": "close-20-api-stopped.json",
    "drained": "close-30-drained.json",
    "worker_stopped": "close-40-worker-stopped.json",
    "verified": "close-50-verified.json",
}
CHECKPOINT_FIELDS = {
    "format_version", "kind", "attempt", "controller", "handoff", "runtime_directory", "scope_id",
    "phase", "previous", "resumed_from", "result", "sha256",
}


class DrainTimeout(TimeoutError):
    def __init__(self, last_snapshot: dict | None, observations: int):
        super().__init__("seed 排空超过固定期限")
        self.last_snapshot, self.observations = last_snapshot, observations


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("排空快照包含重复字段")
        value[key] = item
    return value


def validate_snapshot(value: dict) -> dict:
    exact(value, {"database_time", "active", "terminal_non_success"})
    if not isinstance(value["database_time"], str) or re.fullmatch(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z", value["database_time"]) is None:
        raise ValueError("排空快照数据库时间无效")
    for field, expected in (("active", ACTIVE_QUERIES), ("terminal_non_success", TERMINAL_QUERIES)):
        exact(value[field], set(expected))
        if any(type(item) is not int or item < 0 for item in value[field].values()):
            raise ValueError("排空快照计数必须是非负整数")
    return value


def parse_snapshot(raw: str) -> dict:
    if not isinstance(raw, str) or not raw:
        raise ValueError("排空查询没有返回固定 JSON")
    try:
        value = json.loads(raw, object_pairs_hook=_unique_object)
    except json.JSONDecodeError:
        raise ValueError("排空查询返回无效 JSON") from None
    return validate_snapshot(value)


def is_drained(snapshot: dict) -> bool:
    validate_snapshot(snapshot)
    return not any(snapshot["active"].values())


def snapshot_signature(snapshot: dict) -> tuple:
    validate_snapshot(snapshot)
    return (tuple(snapshot["active"].items()), tuple(snapshot["terminal_non_success"].items()))


def observe_snapshot(context) -> dict:
    with context.environments.use("target"):
        resources = Resources(context.backend, context.target, context.output, context.selected, context.review,
                              context.run, storage_run=context.directory_root)
        databases = [item for item in context.target_config["databases"] if item["key"] == "shared-control"]
        if len(databases) != 1:
            raise ValueError("seed 排空只接受唯一 shared-control 数据库")
        return parse_snapshot(resources.tools.mysql(databases[0], DRAIN_SQL))


def validate_identity(value: dict) -> dict:
    exact(value, {"pid", "started", "executable"})
    if (type(value["pid"]) is not int or value["pid"] <= 1 or not isinstance(value["started"], str)
            or not value["started"].isdigit() or not isinstance(value["executable"], str)
            or not value["executable"]):
        raise ValueError("seed 运行进程身份无效")
    return value


def recorded_identities(runtime: Path, handoff: dict) -> dict:
    return {role: validate_identity(read_process(runtime, role, handoff["scope_id"]))
            for role in ("api", "worker")}


def validate_control(value: dict, operation: str, roles: tuple[str, ...], runtime: Path, handoff: dict) -> dict:
    exact(value, {"format_version", "kind", "operation", "scope_id", "runtime_directory", "processes",
                  "producer_history"})
    expected_history = str(runtime / "producer-history.json")
    if (value["format_version"] != 1 or value["kind"] != "devex-clone-runtime-control"
            or value["operation"] != operation or value["scope_id"] != handoff["scope_id"]
            or value["runtime_directory"] != str(runtime) or value["producer_history"] != expected_history):
        raise ValueError("运行控制结果不属于固定 seed handoff")
    exact(value["processes"], set(roles))
    for process in value["processes"].values():
        exact(process, {"state", "identity", "ready"})
        if process["state"] not in {"running", "stopped"} or type(process["ready"]) is not bool:
            raise ValueError("seed 运行状态无效")
        if process["state"] == "running":
            validate_identity(process["identity"])
        elif process != {"state": "stopped", "identity": None, "ready": False}:
            raise ValueError("已停止 seed 进程仍包含身份或就绪状态")
    return value


def require_initial_running(value: dict, runtime: Path, handoff: dict) -> dict:
    validate_control(value, "status", ("api", "worker"), runtime, handoff)
    for role in ("api", "worker"):
        if value["processes"][role]["state"] != "running" or value["processes"][role]["ready"] is not True:
            raise ValueError("首次 close 必须从同一 handoff 的就绪 API/Worker 开始")
    return value


def require_fixed_running(value: dict, runtime: Path, handoff: dict, identities: dict) -> dict:
    require_initial_running(value, runtime, handoff)
    if any(value["processes"][role]["identity"] != identities[role] for role in ("api", "worker")):
        raise ValueError("运行中的 API/Worker 不属于固定进程代次")
    return value


def require_api_stopped(value: dict, runtime: Path, handoff: dict) -> dict:
    validate_control(value, "stop", ("api",), runtime, handoff)
    if value["processes"]["api"] != {"state": "stopped", "identity": None, "ready": False}:
        raise ValueError("API 停止结果无效")
    return value


def require_worker_stopped(value: dict, runtime: Path, handoff: dict) -> dict:
    validate_control(value, "stop", ("worker",), runtime, handoff)
    if value["processes"]["worker"] != {"state": "stopped", "identity": None, "ready": False}:
        raise ValueError("Worker 停止结果无效")
    return value


def require_drain_runtime(value: dict, runtime: Path, handoff: dict, worker_identity: dict) -> dict:
    validate_control(value, "status", ("api", "worker"), runtime, handoff)
    expected_api = {"state": "stopped", "identity": None, "ready": False}
    worker = value["processes"]["worker"]
    if (value["processes"]["api"] != expected_api or worker["state"] != "running"
            or worker["ready"] is not True or worker["identity"] != worker_identity):
        raise ValueError("排空期间 API/Worker 进程代次不属于固定阶段")
    return value


def validate_observed_runtime(value: dict, runtime: Path, handoff: dict, identities: dict) -> dict:
    validate_control(value, "status", ("api", "worker"), runtime, handoff)
    for role in ("api", "worker"):
        process = value["processes"][role]
        if process["state"] == "running" and (process["ready"] is not True
                or process["identity"] != identities[role]):
            raise ValueError("实际运行进程不属于固定 API/Worker 代次")
    return value


def require_closed_runtime(value: dict, runtime: Path, handoff: dict) -> dict:
    validate_control(value, "status", ("api", "worker"), runtime, handoff)
    expected = {"state": "stopped", "identity": None, "ready": False}
    if value["processes"] != {"api": expected, "worker": expected}:
        raise ValueError("seed 终检要求 API/Worker 均精确停止")
    return value


def drain_until_stable(observe, runtime_status, *, timeout_seconds=DRAIN_TIMEOUT_SECONDS,
                       stable_seconds=DRAIN_STABLE_SECONDS, poll_seconds=DRAIN_POLL_SECONDS,
                       clock=time.monotonic, wait=time.sleep) -> dict:
    if (isinstance(timeout_seconds, bool) or timeout_seconds <= 0 or isinstance(stable_seconds, bool)
            or stable_seconds <= 0 or isinstance(poll_seconds, bool) or poll_seconds <= 0):
        raise ValueError("排空期限和观察间隔必须为正数")
    deadline = clock() + timeout_seconds
    signature, first, last, first_at, observations = None, None, None, None, 0
    while True:
        runtime_status()
        current = validate_snapshot(observe())
        runtime_status()
        observations += 1
        now = clock()
        current_signature = snapshot_signature(current) if is_drained(current) else None
        if current_signature is None:
            signature, first, first_at = None, None, None
        elif current_signature != signature:
            signature, first, first_at = current_signature, current, now
        elif first_at is not None and now - first_at >= stable_seconds:
            return {"stable_seconds": stable_seconds, "observations": observations,
                    "first_snapshot": first, "last_snapshot": current}
        last = current
        if now >= deadline:
            raise DrainTimeout(last, observations)
        wait(min(poll_seconds, max(0, deadline - now)))


def fixed_guard(context, request, runtime: Path, handoff: dict, handoff_binding: dict) -> None:
    from devex_clone_quota_model import capacity_evidence
    from devex_clone_seed import verified_identity_stage
    from devex_clone_seed_runtime import current_guard, runtime_inputs

    observed_runtime, observed_handoff, _ = runtime_inputs(context.backend, context.directory_root)
    if (observed_runtime != runtime or observed_handoff != handoff
            or binding(runtime / "handoff.json") != handoff_binding):
        raise ValueError("close 期间 seed handoff 或 runtime 发生变化")
    if verified_identity_stage(context.backend, context.directory_root, request) != handoff["identity"]:
        raise ValueError("close 期间固定身份验证证据发生变化")
    if capacity_evidence(context.backend, context.directory_root, request) != handoff["capacity"]:
        raise ValueError("close 期间容量证据发生变化")
    if current_guard(context, request) != handoff["schedules"]:
        raise ValueError("close 期间调度确认或无生产者约束发生变化")


def _control(private: dict, backend: Path, runtime: Path, operation: str, roles: tuple[str, ...], handoff: dict) -> dict:
    from devex_clone_runtime import control

    with Environments(private, private).use("target"):
        return control(backend, runtime, operation, roles, handoff["api_url"])


def _drain_evidence(value: dict, worker_identity: dict) -> dict:
    exact(value, {"stable_seconds", "observations", "first_snapshot", "last_snapshot"})
    if (value["stable_seconds"] != DRAIN_STABLE_SECONDS or type(value["observations"]) is not int
            or value["observations"] < 2):
        raise ValueError("稳定排空证据参数无效")
    first, last = validate_snapshot(value["first_snapshot"]), validate_snapshot(value["last_snapshot"])
    if not is_drained(first) or not is_drained(last) or snapshot_signature(first) != snapshot_signature(last):
        raise ValueError("稳定排空证据仍有活跃状态或前后变化")
    validate_identity(worker_identity)
    return value


def _base_result(runtime: Path, handoff: dict, runtime_status: dict, identities: dict, *,
                 phase="running", api_stop=None) -> dict:
    return {
        "format_version": 1, "kind": "devex-clone-seed-close", "status": "needs_reconciliation",
        "phase": phase, "handoff": binding(runtime / "handoff.json"),
        "runtime_directory": str(runtime), "scope_id": handoff["scope_id"],
        "process_identities": identities, "runtime_status": runtime_status, "api_stop": api_stop,
        "drain": None, "worker_stop": None,
        "final_snapshot": None, "last_snapshot": None, "terminal_non_success": None,
        "stopped_evidence": None, "worker_preserved": True, "reason": None, "error_type": None,
        "outbox_drained": False, "direct_remote_writes": 0, "worker_drain_writes": True,
        "restore_qualified": False,
    }


def _partial(result: dict, phase: str, reason: str, error: BaseException | None = None, *,
             snapshot: dict | None = None) -> dict:
    value = dict(result)
    value.update(status="needs_reconciliation", phase=phase, reason=reason,
                 error_type=type(error).__name__ if error else None, outbox_drained=False,
                 worker_preserved=phase in {"running", "api_stopped", "drained"})
    if snapshot is not None:
        value["last_snapshot"] = validate_snapshot(snapshot)
        value["terminal_non_success"] = snapshot["terminal_non_success"]
    return value


def validate_result(value: dict, runtime: Path, handoff: dict, *, in_progress: bool = False) -> dict:
    exact(value, RESULT_FIELDS)
    if (value["format_version"] != 1 or value["kind"] != "devex-clone-seed-close"
            or value["handoff"] != binding(runtime / "handoff.json") or value["runtime_directory"] != str(runtime)
            or value["scope_id"] != handoff["scope_id"] or value["direct_remote_writes"] != 0
            or value["worker_drain_writes"] is not True or value["restore_qualified"] is not False):
        raise ValueError("历史 close 结果不属于固定 handoff")
    exact(value["process_identities"], {"api", "worker"})
    identities = {role: validate_identity(value["process_identities"][role]) for role in ("api", "worker")}
    runtime_status = validate_observed_runtime(value["runtime_status"], runtime, handoff, identities)
    worker_identity = identities["worker"]
    phase = value["phase"]
    if phase not in {"running", "api_stopped", "drained", "worker_stopped", "verified"}:
        raise ValueError("历史 close 阶段无效")
    if phase == "running":
        require_fixed_running(runtime_status, runtime, handoff, identities)
        if value["api_stop"] is not None:
            raise ValueError("运行阶段不能包含 API 停止结果")
    elif value["api_stop"] is None:
        require_drain_runtime(runtime_status, runtime, handoff, worker_identity)
    else:
        require_api_stopped(value["api_stop"], runtime, handoff)
    if phase in {"running", "api_stopped"}:
        if value["drain"] is not None or value["worker_stop"] is not None or value["final_snapshot"] is not None:
            raise ValueError("close 当前阶段包含未来阶段证据")
    else:
        _drain_evidence(value["drain"], worker_identity)
    if phase in {"running", "api_stopped", "drained"} and value["worker_stop"] is not None:
        raise ValueError("Worker 尚未停止却包含停止结果")
    if value["worker_stop"] is not None:
        require_worker_stopped(value["worker_stop"], runtime, handoff)
    if value["final_snapshot"] is not None:
        validate_snapshot(value["final_snapshot"])
    if value["last_snapshot"] is not None:
        validate_snapshot(value["last_snapshot"])
    source = value["final_snapshot"] or value["last_snapshot"]
    if value["terminal_non_success"] != (source["terminal_non_success"] if source else None):
        raise ValueError("历史 close 终态异常统计不属于最后快照")
    expected_preserved = phase in {"running", "api_stopped", "drained"}
    if value["worker_preserved"] is not expected_preserved:
        raise ValueError("历史 close Worker 保留语义无效")
    if value["status"] == "seed_runtime_closed":
        if (phase != "verified" or value["reason"] is not None or value["error_type"] is not None
                or value["outbox_drained"] is not True or value["final_snapshot"] is None
                or not is_drained(value["final_snapshot"]) or value["stopped_evidence"] is None):
            raise ValueError("历史 close 成功结果不完整")
    elif (value["status"] != "needs_reconciliation" or phase == "verified"
          or value["reason"] is None and (not in_progress or value["error_type"] is not None)
          or value["reason"] is not None and value["reason"] not in REASONS
          or value["outbox_drained"] is not False or (value["error_type"] is not None and
          (not isinstance(value["error_type"], str) or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", value["error_type"]) is None))):
        raise ValueError("历史 close 待核对结果无效")
    return value


def _original_attempt(attempt: dict) -> dict:
    return {**attempt, "status": "running", "finished_at": None, "result": None, "error_type": None}


def _validate_controller(backend: Path, directory: Path, attempt: dict, descriptor: dict) -> None:
    path = bound_file(backend, descriptor)
    expected = directory / f"controller-{attempt['number']:04d}.json"
    value = read_json(path)
    exact(value, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    exact(value["owner"], {"format_version", "identity", "directory", "manifest_sha256"})
    if (path != expected or value["format_version"] != 1 or value["kind"] != "devex-stage-controller"
            or value["attempt"] != attempt["number"] or value["attempt_sha256"] != plan_hash(_original_attempt(attempt))
            or value["owner"]["format_version"] != 1 or value["owner"]["directory"] != str(directory)
            or value["owner"]["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]):
        raise ValueError("close checkpoint 控制器不属于固定 attempt")
    validate_identity(value["owner"]["identity"])


def _checkpoint_body(value: dict) -> dict:
    return {key: item for key, item in value.items() if key != "sha256"}


def _checkpoint_path(directory: Path, attempt: int, phase: str) -> Path:
    return directory / "seed-runtime" / f"attempt-{attempt:04d}" / CHECKPOINT_FILES[phase]


def _validate_resume_source(backend: Path, directory: Path, state: dict, descriptor: dict,
                            current_attempt: int, runtime: Path, handoff: dict) -> None:
    path = bound_file(backend, descriptor)
    result_match = [item for item in state["attempts"] if item["number"] < current_attempt
                    and item["stage"] == "seed-runtime" and item["mode"] == "close"
                    and item["result"] == descriptor]
    if path.parent == directory / "results" and len(result_match) == 1:
        validate_result(read_json(path), runtime, handoff)
        return
    checkpoint_match = [item for item in state["attempts"] if item["number"] < current_attempt
                        and item["stage"] == "seed-runtime" and item["mode"] == "close"
                        and path.parent == directory / "seed-runtime" / f"attempt-{item['number']:04d}"
                        and path.name in CHECKPOINT_FILES.values()]
    if len(checkpoint_match) != 1:
        raise ValueError("close checkpoint 续作来源不属于更早的固定 close")
    loaded = _load_checkpoint_chain(backend, directory, state, checkpoint_match[0], runtime, handoff)
    if loaded is None or loaded[1] != descriptor:
        raise ValueError("close checkpoint 续作来源不是原 attempt 的最新完整链")


def _validate_checkpoint(backend: Path, directory: Path, state: dict, attempt: dict, value: dict,
                         phase: str, previous: dict | None, runtime: Path, handoff: dict) -> dict:
    exact(value, CHECKPOINT_FIELDS)
    body = _checkpoint_body(value)
    if (value["format_version"] != 1 or value["kind"] != "devex-clone-seed-close-checkpoint"
            or value["attempt"] != attempt["number"] or value["phase"] != phase
            or value["previous"] != previous or value["sha256"] != plan_hash(body)
            or value["handoff"] != binding(runtime / "handoff.json")
            or value["runtime_directory"] != str(runtime) or value["scope_id"] != handoff["scope_id"]):
        raise ValueError("close checkpoint 不属于固定阶段或摘要链")
    _validate_controller(backend, directory, attempt, value["controller"])
    result = validate_result(value["result"], runtime, handoff, in_progress=True)
    if result["phase"] != phase:
        raise ValueError("close checkpoint 阶段与结果不同")
    if previous is None:
        if value["resumed_from"] is not None:
            _validate_resume_source(backend, directory, state, value["resumed_from"], attempt["number"],
                                    runtime, handoff)
        elif phase not in {"running", "api_stopped"}:
            raise ValueError("close checkpoint 中途阶段缺少固定续作来源")
    elif value["resumed_from"] is not None:
        raise ValueError("close checkpoint 只有链首可以绑定续作来源")
    return result


def _load_checkpoint_chain(backend: Path, directory: Path, state: dict, attempt: dict,
                           runtime: Path, handoff: dict) -> tuple[dict, dict] | None:
    if attempt["stage"] != "seed-runtime" or attempt["mode"] != "close":
        raise ValueError("close checkpoint 不属于 close attempt")
    present = [(phase, _checkpoint_path(directory, attempt["number"], phase))
               for phase in CHECKPOINT_FILES if _checkpoint_path(directory, attempt["number"], phase).exists()]
    if not present:
        return None
    indexes = [list(CHECKPOINT_FILES).index(phase) for phase, _path in present]
    if indexes != list(range(indexes[0], indexes[-1] + 1)):
        raise ValueError("close checkpoint 阶段链不连续")
    previous = None
    result = None
    for phase, path in present:
        value = read_json(path)
        result = _validate_checkpoint(backend, directory, state, attempt, value, phase, previous,
                                      runtime, handoff)
        previous = binding(path)
    return result, previous


def _write_checkpoint(path: Path, value: dict) -> dict:
    if path.exists():
        if read_json(path) != value:
            raise ValueError("close checkpoint 已存在且内容不同")
        return binding(path)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)
    except FileExistsError:
        if read_json(path) != value:
            raise ValueError("close checkpoint 发布竞态产生不同内容") from None
    finally:
        temporary.unlink(missing_ok=True)
    return binding(path)


def publish_checkpoint(context, number: int, result: dict, runtime: Path, handoff: dict,
                       *, resumed_from: dict | None = None) -> dict:
    state = load_state(context.directory_root)
    if (not state["attempts"] or state["attempts"][-1]["number"] != number
            or state["attempts"][-1]["stage"] != "seed-runtime"
            or state["attempts"][-1]["mode"] != "close" or state["attempts"][-1]["status"] != "running"
            or context.output != context.directory_root / "seed-runtime" / f"attempt-{number:04d}"):
        raise ValueError("close checkpoint 只能由当前持锁 attempt 发布")
    attempt = state["attempts"][-1]
    loaded = _load_checkpoint_chain(context.backend, context.directory_root, state, attempt, runtime, handoff)
    previous = loaded[1] if loaded else None
    if loaded is not None and resumed_from is not None:
        raise ValueError("close checkpoint 已有链时不能更换续作来源")
    phase = result["phase"]
    if phase not in CHECKPOINT_FILES:
        raise ValueError("close checkpoint 阶段无效")
    existing_phases = [item for item in CHECKPOINT_FILES
                       if _checkpoint_path(context.directory_root, number, item).exists()]
    if existing_phases and list(CHECKPOINT_FILES).index(phase) <= list(CHECKPOINT_FILES).index(existing_phases[-1]):
        raise ValueError("close checkpoint 阶段不能回退或重复")
    body = {
        "format_version": 1, "kind": "devex-clone-seed-close-checkpoint", "attempt": number,
        "controller": binding(context.directory_root / f"controller-{number:04d}.json"),
        "handoff": binding(runtime / "handoff.json"), "runtime_directory": str(runtime),
        "scope_id": handoff["scope_id"], "phase": phase, "previous": previous,
        "resumed_from": resumed_from if previous is None else None, "result": result,
    }
    value = {**body, "sha256": plan_hash(body)}
    _validate_checkpoint(context.backend, context.directory_root, state, attempt, value, phase,
                         previous, runtime, handoff)
    return _write_checkpoint(_checkpoint_path(context.directory_root, number, phase), value)


def previous_result(backend: Path, directory: Path, number: int, runtime: Path, handoff: dict):
    state = load_state(directory)
    attempts = state["attempts"]
    if (not attempts or attempts[-1]["number"] != number or attempts[-1]["stage"] != "seed-runtime"
            or attempts[-1]["mode"] != "close" or attempts[-1]["status"] != "running"):
        raise ValueError("close 必须属于当前统一验收阶段")
    closes = [item for item in attempts[:-1] if item["stage"] == "seed-runtime" and item["mode"] == "close"]
    if not closes:
        return None, None
    prior = closes[-1]
    if any(item["number"] > prior["number"] for item in attempts[:-1]):
        raise ValueError("上次 close 后已有其他阶段，不能猜测进程或任务状态")
    if prior["result"] is None:
        checkpoint = _load_checkpoint_chain(backend, directory, state, prior, runtime, handoff)
        return checkpoint if checkpoint is not None else (MISSING_RESULT, None)
    value = validate_result(read_json(bound_file(backend, prior["result"])), runtime, handoff)
    if prior["status"] == "passed":
        if value["status"] != "seed_runtime_closed":
            raise ValueError("已通过的 close 结果状态无效")
        raise ValueError("seed runtime 已完成 close，禁止重复停止")
    return value, prior["result"]


def _status(backend: Path, runtime: Path, private: dict, handoff: dict) -> dict:
    return _control(private, backend, runtime, "status", ("api", "worker"), handoff)


def _resume(result: dict, backend: Path, runtime: Path, private: dict, handoff: dict) -> tuple[dict, dict]:
    identities = result["process_identities"]
    worker_identity = identities["worker"]
    observed = _status(backend, runtime, private, handoff)
    if result["phase"] == "running":
        try:
            require_fixed_running(observed, runtime, handoff, identities)
        except ValueError:
            require_drain_runtime(observed, runtime, handoff, worker_identity)
            result = dict(result)
            result.update(phase="api_stopped", runtime_status=observed, api_stop=None,
                          reason=None, error_type=None)
    elif result["phase"] == "api_stopped":
        require_drain_runtime(observed, runtime, handoff, worker_identity)
    elif result["phase"] == "drained":
        try:
            require_drain_runtime(observed, runtime, handoff, worker_identity)
        except ValueError:
            require_closed_runtime(observed, runtime, handoff)
            result = dict(result)
            result.update(phase="worker_stopped", worker_stop=None, worker_preserved=False,
                          reason=None, error_type=None)
    else:
        require_closed_runtime(observed, runtime, handoff)
    return result, worker_identity


def _fresh(backend: Path, runtime: Path, private: dict, handoff: dict) -> tuple[dict, dict]:
    status = require_initial_running(_status(backend, runtime, private, handoff), runtime, handoff)
    identities = {role: status["processes"][role]["identity"] for role in ("api", "worker")}
    if recorded_identities(runtime, handoff) != identities:
        raise ValueError("首次 close 的进程收据与实际 API/Worker 代次不同")
    return _base_result(runtime, handoff, status, identities), identities["worker"]


def _reconcile_missing(backend: Path, runtime: Path, private: dict, handoff: dict) -> tuple[dict, dict]:
    identities = recorded_identities(runtime, handoff)
    observed = validate_observed_runtime(_status(backend, runtime, private, handoff), runtime, handoff, identities)
    try:
        require_fixed_running(observed, runtime, handoff, identities)
        phase = "running"
    except ValueError:
        require_drain_runtime(observed, runtime, handoff, identities["worker"])
        phase = "api_stopped"
    return _base_result(runtime, handoff, observed, identities, phase=phase), identities["worker"]


def _stop_api(result: dict, backend: Path, runtime: Path, private: dict, handoff: dict) -> dict:
    identities = result["process_identities"]
    try:
        stopped = require_api_stopped(
            _control(private, backend, runtime, "stop", ("api",), handoff), runtime, handoff)
    except Exception as error:
        observed = validate_observed_runtime(_status(backend, runtime, private, handoff), runtime, handoff, identities)
        try:
            require_fixed_running(observed, runtime, handoff, identities)
            value = dict(result)
            value["runtime_status"] = observed
            return _partial(value, "running", "api_stop_failed", error)
        except ValueError:
            require_drain_runtime(observed, runtime, handoff, identities["worker"])
            value = dict(result)
            value.update(phase="api_stopped", runtime_status=observed, api_stop=None, reason=None, error_type=None)
            return value
    value = dict(result)
    value.update(phase="api_stopped", api_stop=stopped, reason=None, error_type=None)
    try:
        observed = require_drain_runtime(
            _status(backend, runtime, private, handoff), runtime, handoff, identities["worker"])
    except Exception as error:
        return _partial(value, "api_stopped", "post_api_stop_observation_failed", error)
    value["runtime_status"] = observed
    return value


def _drain(context, result: dict, backend: Path, runtime: Path, private: dict, handoff: dict,
           worker_identity: dict) -> dict:
    def status():
        observed = _status(backend, runtime, private, handoff)
        return require_drain_runtime(observed, runtime, handoff, worker_identity)

    try:
        evidence = drain_until_stable(lambda: observe_snapshot(context), status)
    except DrainTimeout as error:
        status()
        return _partial(result, "api_stopped", "drain_timeout", error, snapshot=error.last_snapshot)
    except Exception as error:
        status()
        return _partial(result, "api_stopped", "drain_observation_failed", error)
    value = dict(result)
    value.update(phase="drained", drain=evidence, last_snapshot=evidence["last_snapshot"],
                 terminal_non_success=evidence["last_snapshot"]["terminal_non_success"])
    return value


def _stop_worker(context, request, result: dict, backend: Path, runtime: Path, private: dict,
                 handoff: dict, worker_identity: dict, handoff_binding: dict) -> dict:
    try:
        fixed_guard(context, request, runtime, handoff, handoff_binding)
        require_drain_runtime(_status(backend, runtime, private, handoff), runtime, handoff, worker_identity)
    except Exception as error:
        require_drain_runtime(_status(backend, runtime, private, handoff), runtime, handoff, worker_identity)
        return _partial(result, "drained", "guard_failed", error, snapshot=result["last_snapshot"])
    try:
        stopped = require_worker_stopped(
            _control(private, backend, runtime, "stop", ("worker",), handoff), runtime, handoff)
    except Exception as error:
        observed = _status(backend, runtime, private, handoff)
        try:
            require_drain_runtime(observed, runtime, handoff, worker_identity)
            return _partial(result, "drained", "worker_stop_failed", error, snapshot=result["last_snapshot"])
        except ValueError:
            require_closed_runtime(observed, runtime, handoff)
            value = _partial(result, "worker_stopped", "worker_stop_result_unavailable", error,
                             snapshot=result["last_snapshot"])
            value["worker_stop"] = None
            return value
    value = dict(result)
    value.update(phase="worker_stopped", worker_stop=stopped, worker_preserved=False)
    return value


def _finalize(context, request, result: dict, runtime: Path, handoff: dict, private: dict,
              handoff_binding: dict) -> dict:
    from devex_clone_seed_runtime import stopped_evidence

    snapshot = None
    try:
        require_closed_runtime(_status(context.backend, runtime, private, handoff), runtime, handoff)
        snapshot = observe_snapshot(context)
        fixed_guard(context, request, runtime, handoff, handoff_binding)
        evidence = stopped_evidence(runtime, handoff)
    except Exception as error:
        return _partial(result, "worker_stopped", "final_verification_failed", error,
                        snapshot=snapshot or result.get("final_snapshot") or result.get("last_snapshot"))
    value = dict(result)
    value.update(final_snapshot=snapshot, last_snapshot=snapshot,
                 terminal_non_success=snapshot["terminal_non_success"], stopped_evidence=evidence,
                 worker_preserved=False)
    if not is_drained(snapshot):
        return _partial(value, "worker_stopped", "final_state_not_drained", snapshot=snapshot)
    value.update(status="seed_runtime_closed", phase="verified", reason=None, error_type=None,
                 outbox_drained=True)
    return value


def execute_close(context, request, number: int) -> dict:
    from devex_clone_seed_runtime import runtime_inputs

    runtime, handoff, private = runtime_inputs(context.backend, context.directory_root)
    handoff_binding = binding(runtime / "handoff.json")
    fixed_guard(context, request, runtime, handoff, handoff_binding)
    prior, resumed_from = previous_result(context.backend, context.directory_root, number, runtime, handoff)
    if prior is None:
        result, worker_identity = _fresh(context.backend, runtime, private, handoff)
    elif prior is MISSING_RESULT:
        result, worker_identity = _reconcile_missing(context.backend, runtime, private, handoff)
    else:
        result, worker_identity = _resume(prior, context.backend, runtime, private, handoff)
        if result["status"] == "seed_runtime_closed":
            result["phase"] = "worker_stopped"
            result["status"], result["outbox_drained"] = "needs_reconciliation", False
    publish_checkpoint(context, number, result, runtime, handoff, resumed_from=resumed_from)
    if result["phase"] == "running":
        result = _stop_api(result, context.backend, runtime, private, handoff)
        if result["phase"] == "running" or result["reason"] == "post_api_stop_observation_failed":
            return validate_result(result, runtime, handoff)
        publish_checkpoint(context, number, result, runtime, handoff)
    if result["phase"] == "api_stopped":
        result = _drain(context, result, context.backend, runtime, private, handoff, worker_identity)
        if result["status"] == "needs_reconciliation" and result["phase"] == "api_stopped":
            return validate_result(result, runtime, handoff)
        publish_checkpoint(context, number, result, runtime, handoff)
    if result["phase"] == "drained":
        result = _stop_worker(context, request, result, context.backend, runtime, private, handoff,
                              worker_identity, handoff_binding)
        if result["phase"] == "drained":
            return validate_result(result, runtime, handoff)
        publish_checkpoint(context, number, result, runtime, handoff)
    result = validate_result(_finalize(context, request, result, runtime, handoff, private, handoff_binding),
                             runtime, handoff)
    if result["phase"] == "verified":
        publish_checkpoint(context, number, result, runtime, handoff)
    return result

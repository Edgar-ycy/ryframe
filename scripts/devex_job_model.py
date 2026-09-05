"""真实任务测量的关联与时间证据校验，不从可变更新时间推断执行时间。"""
from __future__ import annotations

from datetime import datetime
import re


JOB_TYPES = {"export": "system.export.execute", "import": "system.user.import",
             "message": "system.message.dispatch"}
OUTCOMES = {"succeeded", "failed", "dead", "deferred", "lease_expired", "running"}


class EvidenceError(ValueError):
    """仅携带固定分类，避免把连接信息或任务载荷写入报告。"""


def identifier(value: object) -> str:
    if (not isinstance(value, str) or not re.fullmatch(r"[1-9][0-9]{0,18}", value)
            or int(value) > 2**63 - 1):
        raise EvidenceError("invalid_job_identifier")
    return value


def selection(value: dict) -> dict:
    if not isinstance(value, dict) or set(value) != {"kind", "id", "tenant_id", "job_type"}:
        raise EvidenceError("invalid_job_selection")
    kind = value["kind"]
    if kind not in {*JOB_TYPES, "schedule"}:
        raise EvidenceError("invalid_job_kind")
    identifier(value["id"])
    if not isinstance(value["tenant_id"], str) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", value["tenant_id"]):
        raise EvidenceError("invalid_tenant")
    if (not isinstance(value["job_type"], str)
            or not re.fullmatch(r"[a-z][a-z0-9_.-]{0,95}", value["job_type"])
            or (kind in JOB_TYPES and value["job_type"] != JOB_TYPES[kind])):
        raise EvidenceError("wrong_job_type")
    return value


def associated_job(rows: list[dict], expected: dict) -> dict:
    selection(expected)
    if not rows:
        raise EvidenceError("missing_job")
    if len(rows) != 1:
        raise EvidenceError("ambiguous_job")
    row = rows[0]
    identifier(row.get("job_id"))
    if row.get("business_id") != expected["id"]:
        raise EvidenceError("wrong_business_identifier")
    if row.get("business_tenant") != expected["tenant_id"]:
        raise EvidenceError("wrong_tenant")
    platform_schedule = (expected["kind"] == "schedule" and expected["tenant_id"] == "system"
                         and row.get("job_tenant") is None)
    if row.get("job_tenant") != expected["tenant_id"] and not platform_schedule:
        raise EvidenceError("wrong_job_tenant")
    if row.get("job_type") != expected["job_type"]:
        raise EvidenceError("wrong_job_type")
    if expected["kind"] == "schedule" and row.get("business_status") != "enqueued":
        raise EvidenceError("schedule_not_enqueued")
    if expected["kind"] == "message" and row.get("business_status") != "published":
        raise EvidenceError("message_not_dispatched")
    return row


def timestamp(value: object) -> datetime:
    if not isinstance(value, str):
        raise EvidenceError("missing_attempt_timestamp")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise EvidenceError("invalid_attempt_timestamp") from error
    if parsed.tzinfo is None:
        raise EvidenceError("timestamp_without_timezone")
    return parsed


def attempt_times(row: dict, attempts: list[dict]) -> dict:
    count = row.get("claim_sequence")
    if not isinstance(count, int) or isinstance(count, bool) or count < 1 or len(attempts) != count:
        raise EvidenceError("missing_attempt_history")
    queues, executions, previous_end = [], [], None
    for sequence, attempt in enumerate(attempts, 1):
        if attempt.get("job_id") != row["job_id"] or attempt.get("sequence") != sequence:
            raise EvidenceError("wrong_attempt_identity")
        outcome = attempt.get("outcome")
        if outcome not in OUTCOMES:
            raise EvidenceError("invalid_attempt_outcome")
        if outcome in {"running", "lease_expired"}:
            raise EvidenceError("attempt_result_unknown" if outcome == "lease_expired" else "unfinished_attempt")
        queued, started, finished, closed = [timestamp(attempt.get(key)) for key in
                                             ("available_at", "started_at", "finished_at", "closed_at")]
        if not queued <= started <= finished == closed or (previous_end and started < previous_end):
            raise EvidenceError("unordered_attempt_timestamps")
        queues.append((started - queued).total_seconds() * 1000)
        executions.append((finished - started).total_seconds() * 1000)
        previous_end = finished
    if attempts[-1]["outcome"] != "succeeded":
        raise EvidenceError("last_attempt_not_successful")
    return {"queue_ms": sum(queues), "execution_ms": sum(executions), "attempt_count": count,
            "unsuccessful_attempts": sum(attempt["outcome"] != "succeeded" for attempt in attempts)}


def evidence(row: dict, attempts: list[dict]) -> dict:
    result = {"job": row, "attempts": attempts, "valid": False}
    try:
        if row.get("business_status") in {"cancelled", "cancel_requested"}:
            raise EvidenceError("job_cancelled")
        if row.get("business_status") in {"failed", "partial", "dead", "expired"} or row.get("status") == "dead":
            raise EvidenceError("job_failed")
        if row.get("status") != "succeeded" or row.get("business_status") not in {"succeeded", "published", "enqueued"}:
            raise EvidenceError("job_unfinished")
        result.update(attempt_times(row, attempts), valid=True)
    except EvidenceError as error:
        result["reason"] = str(error)
    return result

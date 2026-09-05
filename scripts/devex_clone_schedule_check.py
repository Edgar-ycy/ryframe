"""精确核对 seed 调度停用的数据库前后行；不调用 API 或修改 SQL。"""
from __future__ import annotations

from datetime import datetime
from decimal import Decimal
from pathlib import Path

from devex_clone_rows import schema_catalog
from devex_clone_schedule import canonical_value, inspect_schedule, schedule_actions, schedule_row_sha256
from restore_reference_plan import plan_hash


def database_time(value) -> datetime:
    if isinstance(value, str):
        value = datetime.fromisoformat(value)
    if not isinstance(value, datetime) or value.tzinfo is not None:
        raise ValueError("调度核对只接受数据库 UTC 的无时区 DATETIME")
    return value


def normalized(row: dict, columns: dict) -> dict:
    if not isinstance(row, dict) or set(row) != set(columns):
        raise ValueError("必须读取调度当前 schema 的完整数据库行，不接受部分 API 投影")
    result = dict(row)
    for field, kind in columns.items():
        value = row[field]
        if kind == "DATETIME" and value is not None:
            result[field] = database_time(value)
        if kind in {"BIGINT", "INT", "TINYINT"} and value is not None:
            if type(value) not in {int, Decimal} or not Decimal(value).is_finite() or Decimal(value) != int(value):
                raise ValueError("调度数据库整数列不能包含布尔值、非整数或歧义类型")
    return result


def inspect_disabled(backend: Path, action: dict, before: dict, observed: dict,
                     database_started_at, database_finished_at) -> dict:
    """用于成功响应后的确认或超时后的只读核对；相同前像不授予自动重放权限。"""
    columns = schema_catalog(backend)[0]["sys_job_schedule"]
    old, current = normalized(before, columns), normalized(observed, columns)
    actions = schedule_actions({"copy_stage": "source_to_seed", "target_schedule_actions": [action]})
    if inspect_schedule(old, actions) is None or actions:
        raise ValueError("停用核对没有完整匹配原启用调度的计划行")
    started, finished = database_time(database_started_at), database_time(database_finished_at)
    if started > finished:
        raise ValueError("调度变更的数据库时间窗口顺序错误")
    old_hash, observed_hash = schedule_row_sha256(old), schedule_row_sha256(current)
    common = {"action_sha256": plan_hash(action), "before_sha256": old_hash, "observed_sha256": observed_hash,
              "schedule_id": action["schedule_id"], "tenant_id": action["tenant_id"],
              "automatic_retry_allowed": False, "target_ready": False, "api_execution_proven": False}
    if canonical_value(current) == canonical_value(old):
        return {**common, "status": "unchanged_before", "worker_must_remain_stopped": True}
    preserved = set(columns) - {"enabled", "next_run_at", "version", "updated_at"}
    if (any(canonical_value(current[field]) != canonical_value(old[field]) for field in preserved)
            or current["enabled"] != 0 or current["next_run_at"] is not None
            or current["version"] != old["version"] + 1 or current["updated_at"] is None
            or not started <= current["updated_at"] <= finished):
        return {**common, "status": "needs_reconciliation", "worker_must_remain_stopped": True}
    return {**common, "status": "disabled_row_verified", "worker_must_remain_stopped": True}

"""复制计划中的精确目标调度待办；仅生成现有 API 的人工可审阅动作，不执行。"""
from datetime import date, datetime
from decimal import Decimal
import re

from restore_reference_plan import _native, plan_hash

DATE_COLUMNS = {"next_run_at", "last_run_at", "created_at", "updated_at"}
ACTION_FIELDS = {"action", "tenant_id", "schedule_id", "handler_key", "expected_version", "source_row_sha256"}


def canonical_value(value):
    if value is None:
        return ["null"]
    if type(value) is bool:
        return ["boolean", value]
    if isinstance(value, (int, Decimal)):
        number = Decimal(value)
        if not number.is_finite():
            raise ValueError("完整行摘要不能包含非有限数字")
        text = format(number, "f")
        if "." in text:
            text = text.rstrip("0").rstrip(".")
        return ["decimal", "0" if number == 0 else text]
    if isinstance(value, bytes):
        return ["bytes", value.hex()]
    if isinstance(value, datetime):
        if value.tzinfo is not None:
            raise ValueError("SQL UTC 日期必须明确为无时区 DATETIME，不接受混合偏移")
        return ["datetime-utc", value.isoformat(sep=" ", timespec="microseconds")]
    if isinstance(value, date):
        return ["date", value.isoformat()]
    if isinstance(value, str):
        return ["text", value]
    if isinstance(value, list):
        return ["array", [canonical_value(item) for item in value]]
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        return ["object", [[key, canonical_value(value[key])] for key in sorted(value)]]
    raise ValueError("完整行摘要包含浮点数或未支持的歧义类型")


def schedule_row_sha256(row: dict) -> str:
    normalized = dict(row)
    for name in DATE_COLUMNS & set(row):
        value = row[name]
        if value is None:
            continue
        if isinstance(value, str):
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d{1,6})?", value):
                raise ValueError("完整调度行的 UTC 日期格式无效")
            value = datetime.fromisoformat(value)
        if not isinstance(value, datetime):
            raise ValueError("完整调度行的日期类型无效")
        normalized[name] = value
    return plan_hash({"kind": "devex-schedule-source-row-v1", "row": canonical_value(normalized)})


def schedule_actions(value: dict) -> dict:
    actions = _native("copy-schedule-actions", {
        "copy_stage": value["copy_stage"],
        "target_schedule_actions": value["target_schedule_actions"],
    })
    return {(action["tenant_id"], action["schedule_id"]): action for action in actions}


def inspect_schedule(row: dict, actions: dict) -> dict | None:
    if row["enabled"] not in {0, 1} or row["del_flag"] not in {"0", "2"}:
        raise ValueError("来源调度状态无效")
    if row["del_flag"] == "2" or row["enabled"] == 0:
        return None
    key = row["tenant_id"], str(row["id"])
    action = actions.pop(key, None)
    if (action is None or action["handler_key"] != row["handler_key"]
            or action["expected_version"] != row["version"] or action["source_row_sha256"] != schedule_row_sha256(row)):
        raise ValueError("启用调度没有精确匹配的目标处置清单、版本或完整源行摘要")
    return {**action, "operations": ["get_monitor_schedules_by_id", "put_monitor_schedules_by_id_status"],
            "path": {"id": action["schedule_id"]}, "body": {"version": action["expected_version"], "enabled": False},
            "required_permissions": ["monitor:schedule:list", "monitor:schedule:edit"],
            "worker_must_remain_stopped": True, "required_api_jobs_mode": "external", "api_called": False,
            "completion_evidence": "目标作用域及原行摘要核验、GET/PUT版本响应、enabled=false/next_run_at=NULL/version+1、源未变；未知结果先核对，不重放"}

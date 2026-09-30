"""复制计划中的精确目标调度待办；仅生成现有 API 的人工可审阅动作，不执行。"""
from datetime import date, datetime
from decimal import Decimal
import re

from restore_reference_plan import plan_hash

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
    stage, actions = value["copy_stage"], value["target_schedule_actions"]
    if stage not in {"source_to_seed", "seed_to_arm"} or not isinstance(actions, list) or len(actions) > 100:
        raise ValueError("复制阶段或目标调度处置清单无效")
    if actions and stage != "source_to_seed":
        raise ValueError("只有来源到 seed 可以声明目标调度待办；两侧必须复制已完成处置的 seed")
    result = {}
    for action in actions:
        if not isinstance(action, dict) or set(action) != ACTION_FIELDS:
            raise ValueError("目标调度动作字段必须完整且唯一")
        if (action["action"] != "disable_schedule_via_api" or action["tenant_id"] != "system"
                or not isinstance(action["schedule_id"], str) or not re.fullmatch(r"[1-9]\d{0,18}", action["schedule_id"])
                or int(action["schedule_id"]) > 2**63 - 1
                or not isinstance(action["handler_key"], str) or not re.fullmatch(r"[a-z][a-z0-9_.-]{0,95}", action["handler_key"])
                or type(action["expected_version"]) is not int or not 0 < action["expected_version"] < 2**63 - 1
                or not isinstance(action["source_row_sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", action["source_row_sha256"])):
            raise ValueError("仅接受明确 system 记录、版本、处理器与完整源行摘要的停用待办")
        key = action["tenant_id"], action["schedule_id"]
        if key in result:
            raise ValueError("同一目标调度待办重复")
        result[key] = action
    return result


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

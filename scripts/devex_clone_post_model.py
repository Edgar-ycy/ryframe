"""复制后只允许登记的配置差异与完整调度行，不推断新资源或认证信息。"""
import re
from pathlib import Path

from devex_clone_model import exact, linked
from devex_clone_rows import schema_catalog
from devex_clone_schedule import ACTION_FIELDS, schedule_actions, schedule_row_sha256
from devex_clone_schedule_check import normalized
from process_sockets import endpoint


def environment_delta(initial: dict, api: dict, frontend_url: str) -> None:
    old = {key: value for key, value in initial.items() if key.startswith("APP_")}
    new = {key: value for key, value in api.items() if key.startswith("APP_")}
    endpoint(frontend_url)
    if old.get("APP_JOBS_SCHEDULER_ENABLED") != "false":
        raise ValueError("初始配置必须明确关闭 scheduler")
    old["APP_JOBS_SCHEDULER_ENABLED"] = "true"
    old["APP_CORS_ALLOW_ORIGINS"] = frontend_url.rstrip("/")
    if new != old or new.get("APP_ENV") != "test" or new.get("APP_JOBS_MODE") != "external":
        raise ValueError("仅可启用调度路由与登记的管理端 Origin，其余 APP 配置必须相同")
    if api.get("SNOWFLAKE_WORKER_ID") != "1":
        raise ValueError("API-only 必须明确登记 Snowflake worker 1")


def administrator(value: dict, environment: dict) -> None:
    exact(value, {"subject_id", "tenant_id", "username", "password_env", "client_address"})
    if (value["tenant_id"] != "system" or not re.fullmatch(r"[1-9][0-9]{0,18}", value["subject_id"])
            or not re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", value["username"])
            or not re.fullmatch(r"RYFRAME_[A-Z0-9_]+", value["password_env"])
            or not environment.get(value["password_env"])
            or not re.fullmatch(r"198\.19\.20\.(?:[1-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-4])", value["client_address"])):
        raise ValueError("必须登记实际 system 身份及独立认证 ENV，不能默认借用密码")


def exact_directory(path: Path) -> None:
    if not path.is_absolute() or any(linked(part) for part in (path, *path.parents)):
        raise ValueError("验收目录必须是无链接的明确绝对路径")


def action_input(pending: dict) -> dict:
    action = {key: pending[key] for key in ACTION_FIELDS}
    schedule_actions({"copy_stage": "source_to_seed", "target_schedule_actions": [action]})
    return action


def validate_api_row(value: dict, action: dict, enabled: bool, version: int) -> None:
    if (not isinstance(value, dict) or value.get("id") != action["schedule_id"]
            or value.get("handler_key") != action["handler_key"] or value.get("enabled") is not enabled
            or type(value.get("version")) is not int or value.get("version") != version):
        raise ValueError("调度 API 行与明确 ID、handler、状态、版本不一致")


def full_row_sql(backend: Path, action: dict) -> str:
    action_input(action)
    columns = schema_catalog(backend)[0]["sys_job_schedule"]
    fields = []
    for name, kind in columns.items():
        expression = f"DATE_FORMAT(`{name}`, '%Y-%m-%d %H:%i:%s.%f')" if kind == "DATETIME" else f"`{name}`"
        fields += [f"'{name}'", expression]
    row = f"(SELECT JSON_OBJECT({','.join(fields)}) FROM sys_job_schedule WHERE tenant_id='system' AND id={action['schedule_id']})"
    return "SELECT JSON_OBJECT('database_now',DATE_FORMAT(UTC_TIMESTAMP(6),'%Y-%m-%d %H:%i:%s.%f'),'row'," + row + ");"


def validate_before(backend: Path, action: dict, observed: dict) -> dict:
    row = normalized(observed["row"], schema_catalog(backend)[0]["sys_job_schedule"])
    if schedule_row_sha256(row) != action["source_row_sha256"]:
        raise ValueError("写前完整目标行不等于精确源行摘要")
    return row

"""调度历史的父任务关联；仅当前三类平台维护目标可以使用全局任务。"""
from datetime import datetime
from decimal import Decimal
import re

from devex_clone_rows import validate_state


# 与 application/jobs/schedule_targets.rs 及各领域 job type 常量逐项回归核对。
SYSTEM_CLEANUP_JOBS = {
    "system.export_result_cleanup": "system.export.cleanup",
    "system.message_retention_cleanup": "system.message.retention",
    "system.data_retention_cleanup": "system.data_retention.cleanup",
}
PAYLOAD_FIELDS = {"schedule_id", "trigger_kind", "scheduled_for"}
TRIGGERS = {"scheduled", "misfire", "manual"}


def positive_id(value) -> str:
    if type(value) not in {int, Decimal} or isinstance(value, Decimal) and not value.is_finite():
        raise ValueError("平台维护关联必须使用明确整数 ID")
    if value != int(value) or not 0 < value <= 2**63 - 1:
        raise ValueError("平台维护关联 ID 超出正整数范围")
    return str(int(value))


def utc_time(value, *, payload=False) -> datetime:
    # 任务时间来自数据库 UTC DATETIME(6)；JSON 由当前 Chrono UTC 序列化为 Z。
    separator, suffix = ("T", "Z") if payload else (" ", "")
    pattern = rf"\d{{4}}-\d{{2}}-\d{{2}}{separator}\d{{2}}:\d{{2}}:\d{{2}}(?:\.\d{{1,6}})?{suffix}"
    if not isinstance(value, str) or not re.fullmatch(pattern, value):
        raise ValueError("平台维护关联时间必须是完整 UTC 微秒时间，不截断未知精度")
    return datetime.fromisoformat(value.removesuffix("Z"))


def validate_system_cleanup(job: dict, execution: dict, schedule: dict | None) -> None:
    if (schedule is None or schedule["tenant_id"] != "system" or execution["tenant_id"] != "system"
            or job["tenant_id"] is not None
            or schedule["handler_key"] not in SYSTEM_CLEANUP_JOBS
            or job["job_type"] != SYSTEM_CLEANUP_JOBS[schedule["handler_key"]]):
        raise ValueError("全局调度父任务仅允许精确 system 维护目标及其登记 job type")
    schedule_id = positive_id(schedule["id"])
    if (positive_id(job["schedule_id"]) != schedule_id or positive_id(execution["schedule_id"]) != schedule_id
            or positive_id(job["id"]) != positive_id(execution["background_job_id"])):
        raise ValueError("平台维护 schedule、execution 与 parent ID 不一致")
    validate_state("sys_background_job", job)
    validate_state("sys_job_schedule_execution", execution)
    scheduled = utc_time(execution["scheduled_for"])
    if utc_time(job["scheduled_for"]) != scheduled or utc_time(job["completed_at"]) < scheduled:
        raise ValueError("平台维护父任务的计划时间或完成时间不匹配")
    payload = job["payload"]
    if (not isinstance(payload, dict) or set(payload) != PAYLOAD_FIELDS
            or payload["schedule_id"] != schedule_id or execution["trigger_kind"] not in TRIGGERS
            or payload["trigger_kind"] != execution["trigger_kind"]
            or utc_time(payload["scheduled_for"], payload=True) != scheduled):
        raise ValueError("平台维护父任务必须保留当前受控载荷及精确触发时间")


def validate_job_relations(observed: dict) -> None:
    if not observed["attempts"].issubset(observed["jobs"]):
        raise ValueError("任务尝试引用不存在的完整任务")
    for execution in observed["executions"]:
        job = observed["jobs"].get(str(execution["background_job_id"]))
        if job is None:
            raise ValueError("调度历史缺少父任务证据；可能已被保留策略清除，不能推断已闭合")
        schedule = observed["schedules"].get(str(execution["schedule_id"]))
        if (job["tenant_id"] is None or job["job_type"] in SYSTEM_CLEANUP_JOBS.values()
                or schedule is not None and schedule["handler_key"] in SYSTEM_CLEANUP_JOBS):
            validate_system_cleanup(job, execution, schedule)
        elif job["tenant_id"] != execution["tenant_id"]:
            raise ValueError("调度执行与终态父任务不属于同一租户")

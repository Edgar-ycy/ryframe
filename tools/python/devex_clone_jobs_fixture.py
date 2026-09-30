"""真实平台清理 SQL 的安全形状；ID 和时间改为固定测试值，不包含环境绑定。"""
import copy

from devex_clone_schedule import schedule_row_sha256


CLEANUPS = (
    ("system.export_result_cleanup", "system.export.cleanup"),
    ("system.message_retention_cleanup", "system.message.retention"),
    ("system.data_retention_cleanup", "system.data_retention.cleanup"),
)
SQL_TIME = "2026-09-01 00:00:00.123456"
JSON_TIME = "2026-09-01T00:00:00.123456Z"


def add_cleanup(case, index=0, *, trigger="scheduled", terminal="succeeded"):
    """通过现有完整列 fixture 生成三表，不伪造实际运行成功收据。"""
    handler, job_type = CLEANUPS[index]
    schedule_id, job_id = 101 + index, 201 + index
    schedule = case.add("shared-control", "sys_job_schedule", id=schedule_id, tenant_id="system",
                        handler_key=handler, enabled=1, del_flag="0", version=1,
                        next_run_at="2026-09-02 00:00:00", created_at=SQL_TIME, updated_at=SQL_TIME)
    job = case.add("shared-control", "sys_background_job", id=job_id, tenant_id=None, job_type=job_type,
                   status=terminal, completed_at="2026-09-01 00:00:01.000001", schedule_id=schedule_id,
                   scheduled_for=SQL_TIME, payload={"schedule_id": str(schedule_id), "trigger_kind": trigger,
                                                   "scheduled_for": JSON_TIME})
    execution = case.add("shared-control", "sys_job_schedule_execution", id=301 + index, tenant_id="system",
                         schedule_id=schedule_id, background_job_id=job_id, outcome="enqueued",
                         trigger_kind=trigger, scheduled_for=SQL_TIME)
    return schedule, job, execution


def bind_actions(case):
    case.value["target_schedule_actions"] = [
        {"action": "disable_schedule_via_api", "tenant_id": row["tenant_id"], "schedule_id": str(row["id"]),
         "handler_key": row["handler_key"], "expected_version": row["version"],
         "source_row_sha256": schedule_row_sha256(row)}
        for table, row in case.data["shared-control"]
        if table == "sys_job_schedule" and row["enabled"] == 1 and row["del_flag"] == "0"
    ]
    case.refresh()
    return copy.deepcopy(case.value)

"""明确隔离迁移的历史状态查询和时间平移；不更改保留期或迁移状态。"""

from __future__ import annotations

from datetime import datetime, timedelta
import re

from full_stack_migration_mysql import identifier

MIGRATION_TIMES = (
    "prechecked_at",
    "queued_at",
    "quiesced_at",
    "frozen_at",
    "copy_started_at",
    "copy_completed_at",
    "verified_at",
    "cut_over_at",
    "activated_at",
    "succeeded_at",
    "retention_until",
    "created_at",
    "updated_at",
)
ITEM_TIMES = ("copy_started_at", "copied_at", "verified_at", "created_at", "updated_at")
JOB_TIMES = ("available_at", "created_at", "updated_at", "completed_at")
ATTEMPT_TIMES = ("available_at", "started_at", "finished_at", "closed_at")
SHIFT_HOURS = 169


def timeline(alias: str, columns: tuple[str, ...]) -> str:
    fields = [
        f"'{column}',DATE_FORMAT({alias}.{column},'%Y-%m-%dT%H:%i:%s.%fZ')"
        for column in columns
    ]
    return "JSON_OBJECT(" + ",".join(fields) + ")"


def snapshot_sql(
    control: str, source: str, target: str, tenant: str, migration: str
) -> str:
    for name in (control, source, target):
        identifier(name)
    return f"""SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,
      'state',m.state,'source',m.source_target_key,'target',m.target_key,
      'source_mode',m.source_target_mode,'target_mode',m.target_target_mode,
      'source_generation',CAST(m.source_generation AS CHAR),'target_generation',CAST(m.target_generation AS CHAR),
      'fingerprint',m.target_schema_fingerprint,'retention_hours',m.retention_hours,
      'retention_seconds',TIMESTAMPDIFF(SECOND,m.succeeded_at,m.retention_until),
      'remaining_seconds',TIMESTAMPDIFF(SECOND,UTC_TIMESTAMP(6),m.retention_until),
      'intent_clear',m.cancel_requested_at IS NULL AND m.finalize_requested_at IS NULL AND m.error_code IS NULL,
      'job_id',CAST(j.id AS CHAR),'job_status',j.status,'job_attempts',j.attempts,'job_tenant_id',j.tenant_id,
      'job_claim_sequence',CAST(j.claim_sequence AS CHAR),
      'attempt_records',COALESCE((SELECT JSON_ARRAYAGG(JSON_OBJECT('job_id',CAST(a.job_id AS CHAR),
          'sequence',CAST(a.sequence AS CHAR),'outcome',a.outcome,'times',{timeline("a", ATTEMPT_TIMES)}))
          FROM `{control}`.sys_background_job_attempt a WHERE a.job_id=j.id),JSON_ARRAY()),
      'job_migration_id',JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.migration_id')),
      'job_lease_clear',j.lease_owner IS NULL AND j.lease_until IS NULL,
      'item_id',CAST(i.id AS CHAR),'item_state',i.state,'item_source_count',i.source_row_count,
      'item_target_count',i.target_row_count,'source_digest',i.source_digest,'target_digest',i.target_digest,
      'cleanup_state',i.cleanup_state,'cleanup_count',i.cleanup_row_count,
      'placement_matches',p.current_target_key=m.target_key AND p.placement_generation=m.target_generation
          AND p.switch_token=m.switch_token AND p.state='active',
      'target_fence_matches',(SELECT COUNT(*) FROM `{target}`.biz_tenant_fence f WHERE f.tenant_id=m.tenant_id
          AND f.target_key=m.target_key AND f.placement_generation=m.target_generation AND f.switch_token=m.switch_token AND f.state='active'),
      'source_fence_matches',(SELECT COUNT(*) FROM `{source}`.biz_tenant_fence f WHERE f.tenant_id=m.tenant_id
          AND f.target_key=m.source_target_key AND f.placement_generation=m.source_generation AND f.switch_token=m.source_switch_token AND f.state='frozen'),
      'target_slot_matches',(SELECT COUNT(*) FROM `{target}`.biz_tenant_target_slot s WHERE s.slot_id=1 AND s.tenant_id=m.tenant_id
          AND s.placement_generation=m.target_generation AND s.switch_token=m.switch_token),
      'source_slot_matches',(SELECT COUNT(*) FROM `{source}`.biz_tenant_target_slot s WHERE s.slot_id=1 AND s.tenant_id=m.tenant_id
          AND s.placement_generation=m.source_generation AND s.switch_token=m.source_switch_token),
      'source_fences',(SELECT COUNT(*) FROM `{source}`.biz_tenant_fence WHERE tenant_id=m.tenant_id),
      'source_slots',(SELECT COUNT(*) FROM `{source}`.biz_tenant_target_slot WHERE tenant_id=m.tenant_id),
      'source_count',(SELECT COUNT(*) FROM `{source}`.biz_order WHERE tenant_id=m.tenant_id),
      'target_count',(SELECT COUNT(*) FROM `{target}`.biz_order WHERE tenant_id=m.tenant_id),
      'source_total',(SELECT COUNT(*) FROM `{source}`.biz_order),
      'target_total',(SELECT COUNT(*) FROM `{target}`.biz_order),
      'other_migrations',(SELECT COUNT(*) FROM `{control}`.sys_tenant_data_migration x WHERE x.tenant_id=m.tenant_id AND x.id<>m.id),
      'operation_leases',(SELECT COUNT(*) FROM `{control}`.sys_tenant_operation_lease WHERE tenant_id=m.tenant_id),
      'migration_times',{timeline("m", MIGRATION_TIMES)},'item_times',{timeline("i", ITEM_TIMES)},'job_times',{timeline("j", JOB_TIMES)})
      FROM `{control}`.sys_tenant_data_migration m
      JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id AND j.job_type='tenant_data_migration'
      JOIN `{control}`.sys_tenant_data_migration_item i ON i.migration_id=m.id AND i.table_name='biz_order'
      JOIN `{control}`.sys_tenant_data_placement p ON p.tenant_id=m.tenant_id
      WHERE m.id={migration} AND m.tenant_id='{tenant}'"""


def validate_attempts(row: dict) -> list[dict]:
    records = row.get("attempt_records")
    sequence = row.get("job_claim_sequence", "")
    if not isinstance(records, list) or not re.fullmatch(r"[1-9][0-9]*", str(sequence)):
        raise ValueError("任务尝试 ownership 或单调领取序号缺失")
    try:
        ordered = sorted(records, key=lambda item: int(item["sequence"]))
        if len(ordered) != int(sequence) or [
            int(item["sequence"]) for item in ordered
        ] != list(range(1, len(ordered) + 1)):
            raise ValueError("任务尝试序号不完整")
        for item in ordered:
            if (
                set(item) != {"job_id", "sequence", "outcome", "times"}
                or item["job_id"] != row["job_id"]
            ):
                raise ValueError("任务尝试归属不匹配")
            times = item["times"]
            if set(times) != set(ATTEMPT_TIMES):
                raise ValueError("任务尝试时间字段不完整")
            parsed = {
                key: datetime.fromisoformat(value) if value is not None else None
                for key, value in times.items()
            }
            available, started, finished, closed = (
                parsed[key] for key in ATTEMPT_TIMES
            )
            if (
                any(value is None for value in (available, started, closed))
                or started < available
                or closed < started
            ):
                raise ValueError("任务尝试未闭合或时间逆序")
            if item["outcome"] == "lease_expired":
                if finished is not None:
                    raise ValueError("租约失效不能伪造实际完成时间")
            elif (
                item["outcome"] not in {"succeeded", "failed", "dead", "deferred"}
                or finished is None
                or finished != closed
            ):
                raise ValueError("任务尝试结果不符合当前时间语义")
        if ordered[-1]["outcome"] != "succeeded":
            raise ValueError("成功任务缺少最终成功尝试")
    except (KeyError, TypeError, IndexError, ValueError) as error:
        raise ValueError("任务尝试 ownership、领取序号、状态或时间不匹配") from error
    return ordered


def validate_snapshot(
    rows: list[dict], tenant: str, migration: str, finalized: bool = False
) -> dict:
    expected = {
        "migration_id": migration,
        "tenant_id": tenant,
        "source": "dedicated-a",
        "target": "dedicated-b",
        "state": "finalized" if finalized else "retention_pending",
        "retention_hours": 168,
        "source_mode": "dedicated",
        "target_mode": "dedicated",
        "job_tenant_id": tenant,
        "retention_seconds": 168 * 3600,
        "job_status": "succeeded",
        "job_migration_id": migration,
        "job_lease_clear": 1,
        "item_state": "verified",
        "item_source_count": 3,
        "item_target_count": 3,
        "cleanup_state": "cleaned" if finalized else "pending",
        "cleanup_count": 3 if finalized else 0,
        "placement_matches": 1,
        "target_fence_matches": 1,
        "target_slot_matches": 1,
        "source_fence_matches": 0 if finalized else 1,
        "source_fences": 0 if finalized else 1,
        "source_slots": 0 if finalized else 1,
        "source_slot_matches": 0 if finalized else 1,
        "source_count": 0 if finalized else 3,
        "source_total": 0 if finalized else 3,
        "target_count": 3,
        "target_total": 3,
        "other_migrations": 0,
        "operation_leases": 0,
    }
    if not finalized:
        expected["intent_clear"] = 1
    if len(rows) != 1 or any(
        rows[0].get(key) != value for key, value in expected.items()
    ):
        raise ValueError(
            "历史 fixture 的迁移、任务、检查点、放置或独占目标 ownership 不匹配"
        )
    row = rows[0]
    if (
        not re.fullmatch(r"[a-f0-9]{64}", row.get("source_digest", ""))
        or row["source_digest"] != row.get("target_digest")
        or not re.fullmatch(r"[a-f0-9]{64}", row.get("fingerprint", ""))
    ):
        raise ValueError("迁移数据摘要或 schema 指纹不匹配")
    row["attempt_records"] = validate_attempts(row)
    return row


def shift_sql(control: str, row: dict) -> str:
    identifier(control)
    attempts = validate_attempts(row)
    statements = []
    for table, key, columns in (
        ("sys_tenant_data_migration", "migration_id", MIGRATION_TIMES),
        ("sys_tenant_data_migration_item", "item_id", ITEM_TIMES),
        ("sys_background_job", "job_id", JOB_TIMES),
    ):
        assignments = ",".join(
            f"{column}=DATE_SUB({column}, INTERVAL {SHIFT_HOURS} HOUR)"
            for column in columns
        )
        statements.append(
            f"UPDATE `{control}`.{table} SET {assignments} WHERE id={row[key]}"
        )
    assignments = ",".join(
        f"a.{column}=DATE_SUB(a.{column}, INTERVAL {SHIFT_HOURS} HOUR)"
        for column in ATTEMPT_TIMES
    )
    sequences = ",".join(str(int(item["sequence"])) for item in attempts)
    statements.append(
        f"UPDATE `{control}`.sys_background_job_attempt a "
        f"JOIN `{control}`.sys_background_job j ON j.id=a.job_id "
        f"JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id "
        f"SET {assignments} WHERE m.id={row['migration_id']} AND m.tenant_id='{row['tenant_id']}' "
        f"AND j.id={row['job_id']} AND j.tenant_id='{row['tenant_id']}' "
        f"AND j.job_type='tenant_data_migration' AND a.sequence IN ({sequences})"
    )
    return ";\n".join(statements)


def verify_timeline(before: dict, after: dict) -> None:
    if before.keys() != after.keys():
        raise ValueError("时间字段没有完整平移")
    for field, value in before.items():
        expected = (
            datetime.fromisoformat(value) - timedelta(hours=SHIFT_HOURS)
            if value is not None
            else None
        )
        actual = (
            datetime.fromisoformat(after[field]) if after[field] is not None else None
        )
        if actual != expected:
            raise ValueError("迁移、检查点或任务尝试时间没有完整平移")


def verify_shift(before: dict, after: dict) -> None:
    excluded = {
        "remaining_seconds",
        "migration_times",
        "item_times",
        "job_times",
        "attempt_records",
    }
    if {k: v for k, v in before.items() if k not in excluded} != {
        k: v for k, v in after.items() if k not in excluded
    }:
        raise ValueError("历史调整改变了时间以外的迁移内容")
    for key in ("migration_times", "item_times", "job_times"):
        verify_timeline(before[key], after[key])
    for old, new in zip(
        validate_attempts(before), validate_attempts(after), strict=True
    ):
        if {k: v for k, v in old.items() if k != "times"} != {
            k: v for k, v in new.items() if k != "times"
        }:
            raise ValueError("任务尝试时间以外的内容发生变化")
        verify_timeline(old["times"], new["times"])
    if after["remaining_seconds"] >= 0:
        raise ValueError("历史 fixture 未形成真实数据库时钟下的到期状态")

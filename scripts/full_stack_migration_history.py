"""168 小时历史保留期 fixture：人工调整迁移日期，使用真实目标导出，不能作为自然时间或恢复演练证据。"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from full_stack_migration_backup import export_target, register_sql, verify_artifact
from full_stack_migration_gate import validate_selection
from full_stack_migration_history_sql import (
    SHIFT_HOURS,
    shift_sql,
    snapshot_sql,
    validate_snapshot,
    verify_shift,
)
from full_stack_migration_mysql import MysqlSession, bindings, ownership
from full_stack_runtime import verify_runtime
from full_stack_worker import worker_identity


def lock_records(
    session: MysqlSession, control: str, tenant: str, migration: str
) -> None:
    session.execute("SET SESSION time_zone='+00:00'; START TRANSACTION")
    rows = session.execute(
        f"SELECT JSON_OBJECT('id',CAST(id AS CHAR)) FROM `{control}`.sys_tenant_data_migration "
        f"WHERE id={migration} AND tenant_id='{tenant}' FOR UPDATE"
    )
    if rows != [{"id": migration}]:
        raise ValueError("当前租户没有唯一匹配迁移")
    session.execute(
        f"SELECT JSON_OBJECT('id',CAST(j.id AS CHAR)) FROM `{control}`.sys_background_job j "
        f"JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id "
        f"WHERE m.id={migration} AND m.tenant_id='{tenant}' FOR UPDATE"
    )
    session.execute(
        f"SELECT JSON_OBJECT('id',CAST(id AS CHAR)) FROM `{control}`.sys_tenant_data_migration_item "
        f"WHERE migration_id={migration} FOR UPDATE"
    )
    session.execute(
        f"SELECT JSON_OBJECT('job_id',CAST(a.job_id AS CHAR),'sequence',CAST(a.sequence AS CHAR)) "
        f"FROM `{control}`.sys_background_job_attempt a "
        f"JOIN `{control}`.sys_background_job j ON j.id=a.job_id AND j.job_type='tenant_data_migration' "
        f"JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id "
        f"WHERE m.id={migration} AND m.tenant_id='{tenant}' AND j.tenant_id='{tenant}' "
        f"ORDER BY a.sequence FOR UPDATE"
    )


def write_evidence(path: Path, value: dict) -> None:
    with path.open("x", encoding="utf-8") as output:
        output.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def assert_history(directory: Path, contract: dict, row: dict) -> dict:
    history = json.loads(
        (directory / f"device-history-{row['migration_id']}.json").read_text(
            encoding="utf-8"
        )
    )
    if (
        any(history.get(key) != value for key, value in contract.items())
        or history.get("artificial_history") is not True
    ):
        raise ValueError("历史调整收据与当前运行、数据库或迁移不匹配")
    verify_shift(history["before"], row)
    return history


def history_plan(contract: dict, row: dict) -> dict:
    row = validate_snapshot([row], contract["tenant_id"], contract["migration_id"])
    if not 167 * 3600 <= row["remaining_seconds"] <= 168 * 3600:
        raise ValueError("仅允许本次刚完成、保留期尚未结束的迁移构造历史 fixture")
    payload = {
        **contract,
        "artificial_history": True,
        "shift_hours": SHIFT_HOURS,
        "retention_hours": 168,
        "natural_elapsed_7_days": False,
        "before": {
            key: value for key, value in row.items() if key != "remaining_seconds"
        },
    }
    canonical = json.dumps(
        payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()
    return {
        **payload,
        "state": "history-planned",
        "plan_sha256": hashlib.sha256(canonical).hexdigest(),
        "before": row,
    }


def plan_history(session: MysqlSession, query: str, contract: dict) -> dict:
    row = validate_snapshot(
        session.execute(query), contract["tenant_id"], contract["migration_id"]
    )
    return history_plan(contract, row)


def apply_history(
    session: MysqlSession,
    control: str,
    query: str,
    directory: Path,
    contract: dict,
    plan_sha256: str,
) -> dict:
    planned = plan_history(session, query, contract)
    if planned["plan_sha256"] != plan_sha256:
        raise ValueError("历史 fixture plan 与当前精确任务或时间事实不匹配，拒绝写入")
    row = planned["before"]
    evidence = {key: value for key, value in planned.items() if key != "state"}
    # 在提交前保存原始时间；任何后续失败均保留收据并拒绝重复平移。
    write_evidence(directory / f"device-history-{row['migration_id']}.json", evidence)
    session.execute(shift_sql(control, row))
    after = validate_snapshot(
        session.execute(query), contract["tenant_id"], contract["migration_id"]
    )
    verify_shift(row, after)
    session.execute("COMMIT")
    return {**evidence, "state": "historical-expired", "after": after}


def backup_history(
    session: MysqlSession,
    control: str,
    target: dict,
    query: str,
    directory: Path,
    contract: dict,
) -> dict:
    row = validate_snapshot(
        session.execute(query), contract["tenant_id"], contract["migration_id"]
    )
    assert_history(directory, contract, row)
    provider = f"full-stack://{contract['scope_id']}/historical-retention/{row['migration_id']}"
    existing = session.execute(
        f"SELECT JSON_OBJECT('count',COUNT(*)) FROM `{control}`.sys_tenant_data_backup_point "
        f"WHERE id={row['migration_id']} OR provider_ref='{provider}'"
    )
    if existing != [{"count": 0}]:
        raise ValueError("备份 ID 或 provider_ref 已存在，拒绝覆盖")
    # 所有业务写事务均锁定 fence；持有该精确行锁期间，仅此租户的写入等待。
    fence = session.execute(
        f"SELECT JSON_OBJECT('tenant_id',tenant_id,'generation',CAST(placement_generation AS CHAR)) "
        f"FROM `{target['database']}`.biz_tenant_fence WHERE tenant_id='{row['tenant_id']}' "
        f"AND placement_generation={row['target_generation']} AND state='active' "
        f"AND switch_token=(SELECT switch_token FROM `{control}`.sys_tenant_data_migration "
        f"WHERE id={row['migration_id']}) FOR UPDATE"
    )
    if fence != [
        {"tenant_id": row["tenant_id"], "generation": row["target_generation"]}
    ]:
        raise ValueError("导出期间无法取得当前租户的精确写入 fence")
    session.execute("SET @backup_captured_at=UTC_TIMESTAMP(6)")
    artifact = export_target(target, directory, row["migration_id"])
    after = validate_snapshot(
        session.execute(query), contract["tenant_id"], contract["migration_id"]
    )
    assert_history(directory, contract, after)
    verify_artifact(artifact, directory, row["migration_id"])
    evidence = {
        **contract,
        "state": "backup-exported",
        "artificial_history": True,
        "natural_elapsed_7_days": False,
        "restored": False,
        "artifact": artifact,
        "backup_id": row["migration_id"],
        "provider_ref": provider,
        "tenant_write_fence_held": True,
    }
    write_evidence(directory / f"device-backup-{row['migration_id']}.json", evidence)
    session.execute(register_sql(control, row, provider, artifact["sha256"]))
    session.execute("COMMIT")
    return evidence


def verify_cleaned(
    session: MysqlSession, query: str, directory: Path, contract: dict
) -> dict:
    row = validate_snapshot(
        session.execute(query),
        contract["tenant_id"],
        contract["migration_id"],
        finalized=True,
    )
    evidence = json.loads(
        (directory / f"device-backup-{row['migration_id']}.json").read_text(
            encoding="utf-8"
        )
    )
    if any(evidence.get(key) != value for key, value in contract.items()):
        raise ValueError("清理验证的备份收据不匹配")
    verify_artifact(evidence["artifact"], directory, row["migration_id"])
    return {
        **contract,
        "state": "cleaned",
        "artificial_history": True,
        "natural_elapsed_7_days": False,
        "restored": False,
        "snapshot": row,
        "artifact": evidence["artifact"],
    }


def run(
    operation: str,
    backend: Path,
    directory: Path,
    tenant: str,
    migration: str,
    plan_sha256: str | None = None,
) -> dict:
    if operation not in {
        "inspect",
        "verify-cleaned",
        "plan-history",
        "historical-expired",
        "export-backup",
    }:
        raise ValueError("未知历史 fixture 操作")
    if operation == "historical-expired" and (
        plan_sha256 is None or len(plan_sha256) != 64
    ):
        raise ValueError("人工历史写入必须提交先前只读 plan 的 SHA256")
    validate_selection(tenant, migration)
    receipt = verify_runtime(backend, directory)
    if (
        operation not in {"inspect", "verify-cleaned"}
        and worker_identity(directory, receipt) is not None
    ):
        raise ValueError("人工历史调整和备份前必须先停止本次已登记 Worker")
    control, source = bindings(backend, "dedicated-a")
    _, target = bindings(backend, "dedicated-b")
    schemas = [control["database"], source["database"], target["database"]]
    if len(set(schemas)) != 3:
        raise ValueError("历史 fixture 必须使用三个不同的明确 schema")
    with MysqlSession(control) as session:
        server = ownership(session, schemas[0], receipt["scope_id"], "control")
        for schema in schemas[1:]:
            if ownership(session, schema, receipt["scope_id"], "tenant-data") != server:
                raise ValueError("历史 fixture 的 MySQL server UUID 不一致")
        contract = {
            "scope_id": receipt["scope_id"],
            "tenant_id": tenant,
            "migration_id": migration,
            "server_uuid": server,
            "schemas": schemas,
            "configuration_sha256": receipt["configuration_sha256"],
        }
        query = snapshot_sql(*schemas, tenant, migration)
        if operation == "inspect":
            rows = session.execute(query)
            if len(rows) != 1:
                raise ValueError("当前租户没有唯一匹配迁移/任务/检查点")
            rows[0]["attempt_records"].sort(key=lambda item: int(item["sequence"]))
            return {**contract, "state": "observed", "snapshot": rows[0]}
        if operation == "verify-cleaned":
            return verify_cleaned(session, query, directory, contract)
        if operation == "plan-history":
            return plan_history(session, query, contract)
        lock_records(session, schemas[0], tenant, migration)
        if operation == "historical-expired":
            return apply_history(
                session, schemas[0], query, directory, contract, plan_sha256
            )
        if operation == "export-backup":
            return backup_history(
                session, schemas[0], target, query, directory, contract
            )
        raise ValueError("未知历史 fixture 操作")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "operation",
        choices=(
            "inspect",
            "plan-history",
            "historical-expired",
            "export-backup",
            "verify-cleaned",
        ),
    )
    parser.add_argument("--backend-root", type=Path, required=True)
    parser.add_argument("--runtime-dir", type=Path, required=True)
    parser.add_argument("--tenant", required=True)
    parser.add_argument("--migration", required=True)
    parser.add_argument("--plan-sha256")
    args = parser.parse_args()
    print(
        json.dumps(
            run(
                args.operation,
                args.backend_root.resolve(),
                args.runtime_dir.resolve(),
                args.tenant,
                args.migration,
                args.plan_sha256,
            ),
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()

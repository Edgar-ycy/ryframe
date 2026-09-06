"""以明确事务锁定位 Device 复制阶段；只控制测试数据库连接，不修改产品状态。"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path

from full_stack_migration_mysql import MysqlSession, bindings, ownership
from full_stack_runtime import verify_runtime
from full_stack_worker import worker_identity


def validate_selection(tenant: str, migration: str) -> None:
    if not re.fullmatch(r"tenant-[a-f0-9]{8}", tenant):
        raise ValueError("仅接受浏览器本次创建的 tenant-xxxxxxxx 测试租户")
    if not re.fullmatch(r"[1-9][0-9]{0,18}", migration) or int(migration) > 2**63 - 1:
        raise ValueError("迁移 ID 必须是正 i64")


def snapshot_sql(control: str, tenant: str, migration: str) -> str:
    return f"""SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,
        'source',m.source_target_key,'target',m.target_key,'state',m.state,
        'retention_hours',m.retention_hours,'job_status',j.status,'attempts',j.attempts,
        'job_migration_id',JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.migration_id')))
        FROM `{control}`.sys_tenant_data_migration m
        JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id
        WHERE m.id={migration} AND m.tenant_id='{tenant}' AND j.job_type='tenant_data_migration'"""


def validate_pending(rows: list[dict], tenant: str, migration: str) -> dict:
    expected = {
        "migration_id": migration,
        "tenant_id": tenant,
        "source": "shared-control",
        "target": "shared",
        "state": "prechecking",
        "retention_hours": 168,
        "job_status": "pending",
        "attempts": 0,
        "job_migration_id": migration,
    }
    if len(rows) != 1 or rows[0] != expected:
        raise ValueError("迁移并非本次隔离租户的未执行 Device 复制任务")
    return rows[0]


def lock_sql(target: str, tenant: str) -> str:
    return f"""SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;
        START TRANSACTION;
        SELECT id FROM `{target}`.biz_device WHERE tenant_id='{tenant}' FOR UPDATE;
        SELECT JSON_OBJECT('connection_id',CONNECTION_ID())"""


def blocked_sql(
    control: str, target: str, tenant: str, migration: str, connection: int
) -> str:
    return f"""SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,
        'state',m.state,'item_state',i.state,'job_status',j.status,'attempts',j.attempts,
        'source_rows',i.source_row_count,'target_rows',i.target_row_count,
        'blocking_connection_id',b.PROCESSLIST_ID,'waiting_connection_id',r.PROCESSLIST_ID,
        'table_name',l.OBJECT_NAME)
        FROM performance_schema.data_lock_waits w
        JOIN performance_schema.data_locks l ON l.ENGINE=w.ENGINE AND l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID
        JOIN performance_schema.threads b ON b.THREAD_ID=w.BLOCKING_THREAD_ID
        JOIN performance_schema.threads r ON r.THREAD_ID=w.REQUESTING_THREAD_ID
        JOIN `{control}`.sys_tenant_data_migration m ON m.id={migration} AND m.tenant_id='{tenant}'
        JOIN `{control}`.sys_tenant_data_migration_item i ON i.migration_id=m.id AND i.table_name='biz_device'
        JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id
        WHERE b.PROCESSLIST_ID={connection} AND l.OBJECT_SCHEMA='{target}' AND l.OBJECT_NAME='biz_device'
            AND m.state='copying' AND i.state='copying' AND j.status='running'"""


def emit(value: dict) -> None:
    print(json.dumps(value, ensure_ascii=False), flush=True)


def hold(backend: Path, directory: Path, tenant: str, migration: str) -> None:
    validate_selection(tenant, migration)
    receipt = verify_runtime(backend, directory)
    if worker_identity(directory, receipt) is not None:
        raise ValueError("创建测试 gate 前必须先停止本次已登记 Worker")
    control, target = bindings(backend)
    with MysqlSession(control) as observer, MysqlSession(target) as gate:
        server = ownership(
            observer, control["database"], receipt["scope_id"], "control"
        )
        if server != ownership(
            gate, target["database"], receipt["scope_id"], "tenant-data"
        ):
            raise ValueError("两个目标的 MySQL server UUID 不一致")
        validate_pending(
            observer.execute(snapshot_sql(control["database"], tenant, migration)),
            tenant,
            migration,
        )
        counts = observer.execute(
            f"SELECT JSON_OBJECT('source',"
            f"(SELECT COUNT(*) FROM `{control['database']}`.biz_device WHERE tenant_id='{tenant}'),"
            f"'target',(SELECT COUNT(*) FROM `{target['database']}`.biz_device WHERE tenant_id='{tenant}'))"
        )
        if counts != [{"source": 3, "target": 0}]:
            raise ValueError("测试必须有恰好三条源 Device，且目标数据为空")
        connection = gate.execute(lock_sql(target["database"], tenant))[0][
            "connection_id"
        ]
        contract = {
            "scope_id": receipt["scope_id"],
            "tenant_id": tenant,
            "migration_id": migration,
        }
        emit({**contract, "state": "held", "blocking_connection_id": connection})
        for line in sys.stdin:
            command = json.loads(line)
            if command == {"operation": "release"}:
                gate.execute("ROLLBACK")
                emit({**contract, "state": "released"})
                return
            if command != {"operation": "wait-blocked"}:
                raise ValueError("gate 只支持 wait-blocked 与 release")
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                rows = observer.execute(
                    blocked_sql(
                        control["database"],
                        target["database"],
                        tenant,
                        migration,
                        connection,
                    )
                )
                if len(rows) == 1:
                    emit({**contract, "state": "blocked", "proof": rows[0]})
                    break
                if rows:
                    raise ValueError("gate 检测到多个请求，无法证明唯一复制执行")
                time.sleep(0.1)
            else:
                raise TimeoutError("没有观察到 Worker 在本 gate 的 Device 行锁上等待")
        # stdin EOF 同样退出 with，回滚锁事务；没有后台驻留服务。


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-root", type=Path, required=True)
    parser.add_argument("--runtime-dir", type=Path, required=True)
    parser.add_argument("--tenant", required=True)
    parser.add_argument("--migration", required=True)
    args = parser.parse_args()
    hold(
        args.backend_root.resolve(),
        args.runtime_dir.resolve(),
        args.tenant,
        args.migration,
    )


if __name__ == "__main__":
    main()

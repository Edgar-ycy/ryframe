"""为保留期测试实际导出唯一专用目标；不将导出校验当作恢复成功。"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess

from full_stack_migration_mysql import client_invocation, identifier


def export_target(binding: dict, directory: Path, migration: str) -> dict:
    database = identifier(binding["database"])
    artifact = directory / f"business-retention-{migration}.sql"
    command, environment = client_invocation(binding, "mysqldump")
    command += [
        "--single-transaction",
        "--quick",
        "--skip-lock-tables",
        "--no-tablespaces",
        "--set-gtid-purged=OFF",
        "--skip-extended-insert",
        "--complete-insert",
        "--routines",
        "--events",
        "--triggers",
        "--hex-blob",
        "--databases",
        database,
    ]
    # 独占文件保留失败证据；不覆盖旧导出，也不扫描其他 schema。
    with artifact.open("xb") as output:
        result = subprocess.run(
            command,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=subprocess.PIPE,
            timeout=120,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
    if result.returncode:
        raise ValueError("mysqldump 未成功完成；保留失败 SQL 文件，不登记备份")
    size = artifact.stat().st_size
    if not 0 < size <= 64 * 1024 * 1024:
        raise ValueError("专用目标导出大小无效或超过测试 64 MiB 上限")
    content = artifact.read_bytes()
    if (
        b"CREATE TABLE `biz_order`" not in content
        or content.count(b"INSERT INTO `biz_order` ") != 3
    ):
        raise ValueError("导出未包含 业务 crate 结构和三条独立记录")
    return {
        "path": str(artifact),
        "bytes": size,
        "sha256": hashlib.sha256(content).hexdigest(),
        "database": database,
        "method": "mysqldump-single-transaction",
        "restored": False,
    }


def register_sql(control: str, row: dict, provider: str, checksum: str) -> str:
    return f"""INSERT INTO `{control}`.sys_tenant_data_backup_point
        (id,scope,tenant_id,target_key,placement_generation,schema_fingerprint,provider_ref,
         captured_at,checksum,validation_status,validation_detail,retention_until,expires_at,created_by,created_at,updated_at)
        SELECT m.id,'tenant',m.tenant_id,m.target_key,m.target_generation,m.target_schema_fingerprint,
          '{provider}',@backup_captured_at,'{checksum}','valid',
          '历史保留期测试：迁移日期人工平移；目标由 mysqldump 真实导出并校验摘要，未执行恢复验证',
          DATE_ADD(@backup_captured_at,INTERVAL 7 DAY),DATE_ADD(@backup_captured_at,INTERVAL 7 DAY),
          m.operator_id,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6)
        FROM `{control}`.sys_tenant_data_migration m WHERE m.id={row["migration_id"]} AND m.tenant_id='{row["tenant_id"]}'"""


def verify_artifact(artifact: dict, directory: Path, migration: str) -> None:
    expected = directory / f"business-retention-{migration}.sql"
    if artifact.get("path") != str(expected) or not expected.is_file():
        raise ValueError("备份文件不属于当前运行目录和迁移")
    with expected.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if expected.stat().st_size != artifact.get("bytes") or digest != artifact.get(
        "sha256"
    ):
        raise ValueError("备份文件大小或摘要已变化")

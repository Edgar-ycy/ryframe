"""开发复制的完整列、受限 SQL 字面量与可执行状态检查；不运行 SQL。"""
from __future__ import annotations

from decimal import Decimal
from functools import lru_cache
import json
from pathlib import Path
import re

from restore_reference_io import DATA_HEADER, validate_insert
from restore_reference_plan import EXCLUDED_TABLES

EXCLUDED = EXCLUDED_TABLES | {"sys_tenant_data_backup_point", "seaql_migrations"}
EMPTY = {"sys_tenant_operation_lease", "sys_tenant_data_migration", "sys_tenant_data_migration_item"}
STATES = {
    "sys_background_job": {"succeeded", "dead"}, "sys_outbox_event": {"published", "dead"},
    "sys_export_job": {"succeeded", "failed", "cancelled", "expired"},
    "sys_user_import_job": {"succeeded", "partial", "failed", "cancelled"},
    "sys_tenant_config_bundle": {"succeeded", "failed", "expired"},
    "sys_tenant_config_transfer": {"preview_ready", "previewed", "applied", "rolled_back", "failed"},
    "sys_data_retention_run": {"succeeded", "failed"}, "password_reset_requests": {"completed", "expired"},
}
EXECUTION_OUTCOMES = {"enqueued", "skipped_misfire", "skipped_concurrency", "target_unavailable", "invalid_configuration"}
# 新表必须先审查状态和物理引用；机器快照只提供列结构，不能自动扩大复制白名单。
CONTROL_TABLES = set("""
sys_tenant sys_cache_namespace_version sys_dept sys_user password_reset_requests sys_role
sys_permission sys_menu sys_post sys_config sys_dict_type sys_dict_data sys_notice sys_oper_log
sys_login_info sys_user_role sys_role_permission sys_role_dept sys_file sys_background_job
sys_job_schedule sys_job_schedule_execution sys_message sys_message_audience sys_message_recipient
sys_data_retention_run sys_user_import_job sys_user_import_row_result sys_tenant_config_bundle
sys_tenant_config_transfer sys_tenant_config_transfer_item sys_product_plan sys_product_plan_version
sys_product_plan_capability sys_tenant_product_plan sys_tenant_capability_override sys_tenant_provision_request
sys_tenant_operation_lease sys_tenant_data_placement sys_tenant_data_migration sys_tenant_data_migration_item
sys_background_job_attempt sys_outbox_event sys_export_job
""".split())


def schema_catalog(backend: Path) -> tuple[dict, dict]:
    """从当前机器 SQL 快照及租户基线读取完整列，不猜测未知生成业务表。"""
    generated = backend / "crates/ryframe-tenant-db/src/generated/catalog.rs"
    generated_tenant = generated_catalog(backend, generated.read_text(encoding="utf-8"))
    control = parse_schema((backend / "sql/ryframe_config.sql").read_text(encoding="utf-8"))
    if set(control) - EXCLUDED != CONTROL_TABLES:
        raise ValueError("控制库表与已审查复制白名单不同；须先审查新增/删除表的状态及物理引用")
    tenant = parse_schema((backend / "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs").read_text(encoding="utf-8"))
    if set(tenant) != {"biz_tenant_fence", "biz_tenant_target_slot", "ryframe_resource_ownership"}:
        raise ValueError("租户基线含未审查表；须补齐复制白名单，不能过滤未知业务表")
    tenant = {name: columns for name, columns in tenant.items() if name in {"biz_tenant_fence", "biz_tenant_target_slot"}}
    if set(tenant) & set(generated_tenant):
        raise ValueError("生成租户表与固定基线表重复")
    tenant.update(generated_tenant)
    if not set(STATES | {name: None for name in EMPTY}).issubset(control):
        raise ValueError("当前 schema 缺少开发复制必须审查的状态表")
    return {name: columns for name, columns in control.items() if name not in EXCLUDED}, tenant


def generated_catalog(backend: Path, content: str) -> dict:
    """从生成描述符及同名迁移读取全部租户业务表，拒绝猜测任意源码目录。"""
    entries = re.findall(
        r'TenantDataTableDescriptor \{\s*table: "([a-z0-9_]+)",.*?tenant_column: "([a-z0-9_]+)",.*?'
        r'column_types: &\[(.*?)\],.*?has_generated_columns: (true|false),', content, re.S)
    empty = re.search(r"GENERATED_TENANT_DATA_TABLES:[^=]+?=\s*&\[\s*\];", content) is not None
    if empty:
        if entries:
            raise ValueError("空生成租户目录包含描述符")
        return {}
    if not entries:
        raise ValueError("非空生成租户目录缺少可验证描述符")
    result = {}
    for table, tenant_column, raw_types, generated_columns in entries:
        if (not table.startswith("biz_") or table in result or tenant_column != "tenant_id"
                or generated_columns != "false"):
            raise ValueError("生成租户描述符包含未审查表、重复表或不支持的列规则")
        resource = table.removeprefix("biz_")
        generated = backend / "crates/ryframe-tenant-db/src/generated"
        migrations = [
            path for path in (
                generated / f"business_{resource}_migration.rs",
                generated / resource / "migration.rs",
            ) if path.is_file()
        ]
        if len(migrations) != 1:
            raise ValueError("生成租户描述符缺少同名迁移")
        migration = migrations[0]
        ddl = re.search(r'pub const CREATE_TABLE_DDL: &str = r#"(.*?)"#;', migration.read_text(encoding="utf-8"), re.S)
        if ddl is None:
            raise ValueError("生成租户迁移缺少确定性 DDL")
        tables = parse_schema(ddl.group(1))
        if set(tables) != {table}:
            raise ValueError("生成租户迁移包含额外或错误表")
        columns = tables[table]
        types = re.findall(r'"([a-z]+)"', raw_types)
        if len(types) != len(columns) or [kind.lower() for kind in columns.values()] != types:
            raise ValueError("生成租户描述符与迁移列类型不一致")
        result[table] = columns
    return result


def parse_schema(content: str) -> dict:
    result = {}
    for match in re.finditer(r"CREATE TABLE(?: IF NOT EXISTS)? `([a-z0-9_]+)` \((.*?)\n\s*\) ENGINE=", content, re.S):
        columns = dict(re.findall(r"^\s*`([a-z0-9_]+)`\s+([A-Za-z]+)", match[2], re.M))
        if not columns or match[1] in result:
            raise ValueError("schema 表或列重复/缺失")
        result[match[1]] = {name: kind.upper() for name, kind in columns.items()}
    return result


def quoted(value: str, position: int) -> tuple[str, int]:
    output, index = [], position + 1
    escapes = {"0": "\0", "b": "\b", "n": "\n", "r": "\r", "t": "\t", "Z": "\x1a",
               "\\": "\\", "'": "'", '"': '"', "%": "\\%", "_": "\\_"}
    while index < len(value):
        char = value[index]
        if char == "'":
            return "".join(output), index + 1
        if char == "\\":
            index += 1
            if index >= len(value) or value[index] not in escapes:
                raise ValueError("SQL 字符串包含未支持转义")
            char = escapes[value[index]]
        output.append(char)
        index += 1
    raise ValueError("SQL 字符串未结束")


def literals(value: str) -> list:
    result, index = [], 0
    while index < len(value):
        while index < len(value) and value[index].isspace():
            index += 1
        if value.startswith("_binary", index):
            index += len("_binary")
            while index < len(value) and value[index].isspace():
                index += 1
        if index >= len(value):
            raise ValueError("SQL 值缺失")
        if value[index] == "'":
            item, index = quoted(value, index)
        else:
            match = re.match(r"NULL|0x[0-9a-fA-F]+|[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?", value[index:])
            if not match:
                raise ValueError("SQL 不是受限字面量")
            token = match[0]
            item = None if token == "NULL" else bytes.fromhex(token[2:]) if token.startswith("0x") else Decimal(token)
            index += len(token)
        result.append(item)
        while index < len(value) and value[index].isspace():
            index += 1
        if index == len(value):
            break
        if value[index] != "," or index + 1 == len(value):
            raise ValueError("SQL 列值分隔无效")
        index += 1
    return result


def parse_row(line: str, catalog: dict) -> tuple[str, dict]:
    validate_insert(line, set(catalog))
    match = re.fullmatch(r"INSERT INTO `([a-z0-9_]+)` \((.*?)\) VALUES \((.*)\);", line)
    if not match:
        raise ValueError("SQL 行结构无效")
    columns = re.findall(r"`([a-z0-9_]+)`", match[2])
    values = literals(match[3])
    if len(columns) != len(values) or len(columns) != len(set(columns)) or set(columns) != set(catalog[match[1]]):
        raise ValueError("SQL 必须完整且恰好包含当前表的全部列")
    row = dict(zip(columns, values, strict=True))
    for name, kind in catalog[match[1]].items():
        if kind == "JSON" and row[name] is not None:
            row[name] = json.loads(row[name], object_pairs_hook=unique_json, parse_int=Decimal, parse_float=Decimal,
                                   parse_constant=invalid_json_number)
    return match[1], row


def unique_json(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("SQL JSON 包含重复字段")
        result[key] = value
    return result


def invalid_json_number(_value):
    raise ValueError("SQL JSON 包含非有限数字")


@lru_cache(maxsize=128)
def logical_tenant_patterns(tenants: frozenset[str]) -> tuple[tuple[str, re.Pattern], ...]:
    # 只缓存有界的已登记逻辑标识规则，不缓存业务行或检查结论。
    return tuple((tenant, re.compile(r"(?<![A-Za-z0-9_-])" + re.escape(tenant) + r"(?![A-Za-z0-9_-])"))
                 for tenant in sorted(tenants, key=len, reverse=True))


def reject_physical(value, forbidden: list[str], logical_tenants: set[str] = frozenset()) -> None:
    folded = tuple(token.casefold() for token in forbidden)
    rules = logical_tenant_patterns(frozenset(logical_tenants))
    inspect_physical(value, folded, rules)


def inspect_physical(value, forbidden: tuple[str, ...], rules: tuple[tuple[str, re.Pattern], ...]) -> None:
    """递归复用本次调用的不可变规则；每个实际字段仍独立核验，不缓存业务值。"""
    if isinstance(value, bytes):
        value = value.decode("utf-8", errors="strict")
    if isinstance(value, str):
        # 已登记租户 ID 是逻辑标识；也会出现在逻辑对象路径与 JSON 内，不改写它们。
        examined = value
        for tenant, pattern in rules:
            if tenant in examined:
                examined = pattern.sub("<logical-tenant>", examined)
        folded = examined.casefold()
        if any(token in folded for token in forbidden):
            raise ValueError("数据含未解决的来源物理 scope、库或端点引用")
    elif isinstance(value, list):
        for item in value:
            inspect_physical(item, forbidden, rules)
    elif isinstance(value, dict):
        for key, item in value.items():
            inspect_physical(key, forbidden, rules)
            physical = key in {"scope", "scope_id", "storage_scope", "object_scope", "redis_namespace"}
            inspect_physical(item, forbidden, () if physical else rules)


def validate_state(table: str, row: dict) -> None:
    if table in EMPTY:
        raise ValueError("租户租约或迁移历史需单独审查，不能自动取消、删行或复制")
    if table in STATES and row["status"] not in STATES[table]:
        raise ValueError("来源存在未解决的可执行任务或工作流")
    if table in {"sys_background_job", "sys_outbox_event"} and any(row[key] is not None for key in ("lease_owner", "lease_until")):
        raise ValueError("终态任务仍携带租约")
    if table == "sys_background_job" and row["completed_at"] is None:
        raise ValueError("终态任务缺少完成时间")
    if table == "sys_background_job_attempt":
        outcome = row["outcome"]
        if outcome not in {"succeeded", "failed", "dead", "deferred", "lease_expired"} or row["closed_at"] is None:
            raise ValueError("后台任务尝试尚未闭合")
        if (outcome == "lease_expired") != (row["finished_at"] is None):
            raise ValueError("后台任务尝试完成时间语义无效")
    if table == "sys_job_schedule_execution":
        if row["outcome"] not in EXECUTION_OUTCOMES:
            raise ValueError("未知调度执行结果")
        if (row["outcome"] == "enqueued") != (row["background_job_id"] is not None):
            raise ValueError("调度触发历史与后台任务关联不一致")
    if table == "sys_job_schedule" and row["del_flag"] != "2" and row["enabled"] != 0:
        raise ValueError("来源仍有可触发调度计划")
    if table == "sys_export_job" and (row["active_request_fingerprint"] is not None or row["delete_pending_at"] is not None):
        raise ValueError("导出仍有活跃请求或待删除状态")
    if table == "sys_file" and row["upload_status"] != "ready":
        raise ValueError("来源文件仍在上传或待清理")
    if table == "sys_tenant" and row["status"] not in {"enabled", "disabled"}:
        raise ValueError("来源租户仍在开通或失败维护状态")


def rows(filename: Path, catalog: dict, *, only_table: str | None = None):
    with filename.open(encoding="utf-8") as stream:
        if stream.readline() != DATA_HEADER:
            raise ValueError("只接受现有工具规范化的数据转储")
        for line in stream:
            if len(line) > 16 * 1024 * 1024:
                raise ValueError("单行导出超过受限检查大小")
            if only_table is not None and not line.startswith(f"INSERT INTO `{only_table}` "):
                continue
            yield parse_row(line.rstrip("\r\n"), catalog)

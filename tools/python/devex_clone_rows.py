"""消费 Rust 受管 typed NDJSON 行流及执行状态；不在 Python 解码 SQL。"""
from __future__ import annotations

from decimal import Decimal, InvalidOperation
from functools import lru_cache
import copy
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

import native_s3_client as native
from artifact_digests import filesystem_path
from restore_build import file_digest
from restore_reference_plan import EXCLUDED_TABLES, _native, plan_hash

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
def schema_catalog(backend: Path) -> tuple[dict, dict]:
    value = _native("copy-schema-catalog", {"backend": str(backend), "include_full": False})
    return tuple({table: dict(pairs) for table, pairs in value[name].items()}
                 for name in ("control", "tenant"))


def unique_json(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("行流 JSON 包含重复字段")
        result[key] = value
    return result


def invalid_json_number(_value):
    raise ValueError("行流 JSON 包含非有限数字")


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


def _request(operation: str, arguments: dict, *, write=False):
    response, _ = native._read_response(*native._invoke(
        "__source-evidence", {"operation": operation, "write": write, "arguments": arguments},
        subprocess.run, may_write=False))
    if set(response) != {"value"}:
        raise ValueError("行流响应包络无效")
    return response["value"]


def _exact(value, fields):
    if not isinstance(value, dict) or set(value) != set(fields):
        raise ValueError("行流字段缺失或未知")


def _uint(value):
    if type(value) is not int or not 0 <= value <= 2**64 - 1:
        raise ValueError("行流必须使用 u64 整数")
    return value


def _sha(value):
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("行流 SHA256 无效")
    return value


def _resolved(path):
    # 复用既有系统路径转换，统一 resolve 后 Windows 普通与 verbatim 的命名空间。
    return Path(filesystem_path(Path(path).resolve()))


def _descriptor(stream, filename, expected, catalog, catalog_sha, only_table):
    _exact(stream, {"format_version", "kind", "backend", "root", "source", "catalog_sha256",
                    "only_table", "tables", "source_rows", "rows", "artifact", "manifest"})
    if type(stream["format_version"]) is not int or stream["format_version"] != 1 or stream["kind"] != "ryframe-sql-row-stream-v1":
        raise ValueError("行流版本无效")
    for field in ("backend", "root"):
        if not isinstance(stream[field], str) or not Path(stream[field]).is_absolute():
            raise ValueError("行流目录必须是绝对路径")
    backend, root = Path(stream["backend"]), Path(stream["root"])
    if (_resolved(backend) != _resolved(native.ROOT)
            or _resolved(root.parent) != _resolved(backend) / ".local-tests/sql-row-streams"
            or not root.name.startswith("sql-rows-") or root.name == "sql-rows-"
            or any(part in {".", ".."} for part in root.parts)):
        raise ValueError("行流越过固定受管目录")
    source = stream["source"]
    _exact(source, {"path", "bytes", "sha256", "device", "inode"})
    if (not isinstance(source["path"], str) or not Path(source["path"]).is_absolute()
            or _resolved(source["path"]) != _resolved(filename)
            or {key: source[key] for key in expected} != expected):
        raise ValueError("行流未绑定本次原始来源")
    for value in (source, stream["artifact"], stream["manifest"]):
        for key in ("bytes", "device", "inode"):
            _uint(value[key])
        _sha(value["sha256"])
    for field, name in (("artifact", "rows.jsonl"), ("manifest", "manifest.json")):
        _exact(stream[field], {"file", "bytes", "sha256", "device", "inode"})
        if stream[field]["file"] != name:
            raise ValueError("行流产物名称无效")
    if not source["bytes"] or not stream["manifest"]["bytes"]:
        raise ValueError("来源与行流清单不能为空")
    _sha(stream["catalog_sha256"])
    if stream["catalog_sha256"] != catalog_sha or stream["only_table"] != only_table:
        raise ValueError("行流目录或选择范围漂移")
    _exact(stream["tables"], catalog)
    total = sum(_uint(value) for value in stream["tables"].values())
    selected = total if only_table is None else stream["tables"][only_table]
    if _uint(stream["source_rows"]) != total or _uint(stream["rows"]) != selected:
        raise ValueError("行流完整表计数不符")


def _cell(cell):
    if not isinstance(cell, dict):
        raise ValueError("行流单元无效")
    kind = cell.get("kind")
    _exact(cell, {"kind"} if kind == "null" else {"kind", "value"})
    if kind == "null":
        return None
    value = cell["value"]
    if kind == "json":
        return value
    if not isinstance(value, str):
        raise ValueError("行流标量必须为精确文本")
    if kind == "text":
        return value
    if kind == "bytes" and re.fullmatch(r"(?:[0-9a-f]{2})*", value) is not None:
        return bytes.fromhex(value)
    if kind == "number":
        try:
            number = Decimal(value)
        except InvalidOperation:
            raise ValueError("行流数值无效") from None
        if number.is_finite():
            return number
    raise ValueError("行流单元类型或数值无效")


def _wire_row(line, catalog, only_table):
    if not line or len(line.encode("utf-8")) > 128 * 1024 * 1024:
        raise ValueError("行流编码行为空或超过 128MiB")
    value = json.loads(line, object_pairs_hook=unique_json, parse_int=Decimal,
                       parse_float=Decimal, parse_constant=invalid_json_number)
    _exact(value, {"table", "row"})
    table = value["table"]
    if not isinstance(table, str) or table not in catalog or only_table is not None and table != only_table:
        raise ValueError("行流返回未知或未选择的表")
    _exact(value["row"], catalog[table])
    row = {}
    for column, cell in value["row"].items():
        item = _cell(cell)
        if (cell["kind"] == "json" and catalog[table][column] != "JSON"
                or catalog[table][column] == "JSON" and cell["kind"] not in {"json", "null"}):
            raise ValueError("行流 JSON 列类型不符")
        row[column] = item
    return table, row


def _chunks(stream):
    offset, size, whole = 0, stream["artifact"]["bytes"], hashlib.sha256()
    while True:
        value = _request("reference-sql-stream-read", {"stream": stream, "offset": offset})
        _exact(value, {"offset", "next_offset", "eof", "text", "chunk_sha256"})
        if not isinstance(value["text"], str) or type(value["eof"]) is not bool:
            raise ValueError("行流分块类型无效")
        data = value["text"].encode("utf-8")
        next_offset = _uint(value["next_offset"])
        if (_uint(value["offset"]) != offset or len(data) > 4 * 1024 * 1024
                or next_offset != offset + len(data) or next_offset > size
                or value["eof"] != (next_offset == size) or not value["eof"] and next_offset == offset
                or _sha(value["chunk_sha256"]) != hashlib.sha256(data).hexdigest()):
            raise ValueError("行流分块游标、边界或摘要无效")
        whole.update(data)
        yield value["text"]
        offset = next_offset
        if value["eof"]:
            if whole.hexdigest() != stream["artifact"]["sha256"]:
                raise ValueError("行流完整产物摘要不符")
            return


def rows(filename: Path, catalog: dict, *, only_table: str | None = None):
    filename, snapshot = Path(filename), copy.deepcopy(catalog)
    if only_table is not None and only_table not in snapshot:
        raise ValueError("选择表不在完整目录中")
    expected, catalog_sha = file_digest(filename), plan_hash(snapshot)
    stream, root = None, "尚未收到 ready descriptor"
    try:
        stream = _request("reference-sql-stream-create", {
            "backend": str(native.ROOT.resolve()), "path": str(filename.absolute()),
            "expected": expected, "catalog": snapshot, "only_table": only_table}, write=True)
        if isinstance(stream, dict):
            root = stream.get("root", root)
        _descriptor(stream, filename, expected, snapshot, catalog_sha, only_table)
        stream = copy.deepcopy(stream)
        counts, pending = dict.fromkeys(snapshot, 0), ""
        for text in _chunks(stream):
            lines = (pending + text).split("\n")
            pending = lines.pop()
            if len(pending.encode("utf-8")) > 128 * 1024 * 1024:
                raise ValueError("行流编码行超过 128MiB")
            for line in lines:
                table, row = _wire_row(line, snapshot, only_table)
                counts[table] += 1
                if counts[table] > stream["tables"][table]:
                    raise ValueError("行流实际行数超过完整清单")
                yield table, row
        selected = stream["tables"] if only_table is None else {
            table: count if table == only_table else 0 for table, count in stream["tables"].items()}
        if pending or counts != selected or sum(counts.values()) != stream["rows"] or catalog != snapshot:
            raise ValueError("行流 EOF、目录快照或实际行数不符")
        if file_digest(filename) != expected:
            raise ValueError("行流结束后原来源摘要变化")
        if _request("reference-sql-stream-close", {"stream": stream}, write=True) is not True:
            raise ValueError("行流本地清理未确认")
    except GeneratorExit:
        print(f"SQL 行流消费者未完成；保留来源与受管现场：{root}", file=sys.stderr)
        raise
    except Exception as error:
        if type(error) is ValueError:
            reason = str(error)
        elif isinstance(error, json.JSONDecodeError):
            reason = f"行流 JSON 解析失败：第 {error.lineno} 行第 {error.colno} 列"
        else:
            reason = type(error).__name__
        raise ValueError(f"SQL 行流未完成；保留来源与受管现场：{root}；原因={reason}；不自动重试") from error

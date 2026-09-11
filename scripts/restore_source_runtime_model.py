"""来源业务验收收据的纯模型；不连接数据库、Redis 或进程。"""
from __future__ import annotations

import copy
import datetime as dt
from pathlib import Path
import re

from restore_reference_plan import plan_hash


SESSION_TABLE_DELTAS = {
    "sys_login_info": 11,
    "sys_oper_log": 0,
    "sys_outbox_event": 0,
}
NODE_RESULT_FIELDS = {
    "format_version", "kind", "status", "scope_id", "origin_tenant_scope_id",
    "lineage_sha256", "actions", "restore_success", "subjects", "tenants",
    "posts", "files", "source_generation_sha256", "lineage_file_sha256",
}
SUBJECT_FIELDS = {"tenant_id", "user_id", "user_authorization_version"}
LOGIN_ROW_FIELDS = {
    "id", "tenant_id", "user_name", "ipaddr", "login_location", "browser",
    "os", "status", "msg", "login_time",
}


def exact(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"{label}字段缺失或包含未知字段")
    return value


def validate_node_result(
    value: dict,
    lineage: dict,
    start_descriptor: dict,
    lineage_descriptor: dict,
) -> dict:
    """只接受同一 start 与 lineage 的 11 个真实登录身份和完整读取结果。"""
    exact(value, NODE_RESULT_FIELDS, "来源 Node 验收结果")
    expected = {
        "format_version": 1,
        "kind": "restore-source-existing-verification",
        "status": "source_existing_data_verified",
        "scope_id": lineage["verification"]["scope_id"],
        "origin_tenant_scope_id": lineage["scopes"]["origin_tenant_scope_id"],
        "lineage_sha256": plan_hash(lineage),
        "actions": {
            "business": "read_only",
            "objects": "read_only",
            "session": "login_logout",
        },
        "restore_success": False,
        "tenants": lineage["scale"]["tenants"],
        "posts": lineage["scale"]["post_samples"],
        "files": lineage["scale"]["business_objects"],
        "source_generation_sha256": start_descriptor["sha256"],
        "lineage_file_sha256": lineage_descriptor["sha256"],
    }
    if any(value.get(field) != item for field, item in expected.items()):
        raise ValueError("来源 Node 验收没有绑定同一 start、血缘或完整业务样本")
    subjects = value["subjects"]
    tenants = lineage["tenants"]
    if not isinstance(subjects, list) or len(subjects) != len(tenants):
        raise ValueError("来源 Node 验收缺少十一租户的登录主体")
    identities = set()
    for subject, tenant in zip(subjects, tenants, strict=True):
        exact(subject, SUBJECT_FIELDS, "来源登录主体")
        identity = subject.get("tenant_id"), subject.get("user_id")
        if (
            subject.get("tenant_id") != tenant["tenant_id"]
            or not isinstance(subject.get("user_id"), str)
            or re.fullmatch(r"[1-9][0-9]{0,18}", subject["user_id"]) is None
            or int(subject["user_id"]) > 9_223_372_036_854_775_807
            or type(subject.get("user_authorization_version")) is not int
            or subject["user_authorization_version"] < 0
            or subject["user_authorization_version"] > 2_147_483_647
            or identity in identities
        ):
            raise ValueError("来源登录主体与派生租户身份不同")
        identities.add(identity)
    return copy.deepcopy(value)


def authorization_cache_plan(namespace: str, subjects: list[dict]) -> list[dict]:
    """按已登记登录主体构造唯一 33 个 Redis 授权缓存键，不扫描或猜测。"""
    if not isinstance(namespace, str) or re.fullmatch(r"ryframe:\{[a-z0-9][a-z0-9_-]{2,47}\}:", namespace) is None:
        raise ValueError("来源 Redis namespace 不是已登记 scope 的规范前缀")
    rows = []
    seen = set()
    for subject in subjects:
        exact(subject, SUBJECT_FIELDS, "来源登录主体")
        tenant, user = subject["tenant_id"], subject["user_id"]
        if (
            not isinstance(tenant, str)
            or re.fullmatch(r"[a-z0-9][a-z0-9_-]{2,47}", tenant) is None
            or not isinstance(user, str)
            or re.fullmatch(r"[1-9][0-9]{0,18}", user) is None
            or int(user) > 9_223_372_036_854_775_807
            or (tenant, user) in seen
        ):
            raise ValueError("来源登录主体不能生成唯一授权缓存键")
        seen.add((tenant, user))
        tag = "{" + tenant + "}"
        rows.append({
            **copy.deepcopy(subject),
            "keys": {
                "tenant_epoch": namespace + f"ryframe:authorization:{tag}:epoch",
                "user_version": namespace + f"ryframe:authorization:{tag}:user:{user}:version",
                "snapshot": namespace + f"ryframe:authorization:{tag}:user:{user}:snapshots",
            },
        })
    keys = [key for row in rows for key in row["keys"].values()]
    if len(rows) != 11 or len(keys) != 33 or len(set(keys)) != 33:
        raise ValueError("来源验收必须精确绑定十一主体的 33 个授权缓存键")
    return rows


def _instant(value: object, label: str) -> dt.datetime:
    if not isinstance(value, str):
        raise ValueError(f"{label}缺少带时区时间")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError(f"{label}不是有效时间") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError(f"{label}缺少时区")
    return parsed.astimezone(dt.timezone.utc)


def validate_login_audit(
    old_before: bytes,
    old_after: bytes,
    upper_id: int,
    new_rows: list[dict],
    lineage: dict,
    subjects: list[dict],
    started_at: str,
    completed_at: str,
) -> dict:
    """证明旧登录行逐字节不变，且新增行精确来自本次十一租户登录。"""
    if (
        not isinstance(old_before, bytes)
        or old_before != old_after
        or type(upper_id) is not int
        or upper_id < 0
        or not isinstance(new_rows, list)
        or len(new_rows) != 11
    ):
        raise ValueError("来源验收登录日志旧行或新增数量不可信")
    begin, end = _instant(started_at, "来源验收开始"), _instant(completed_at, "来源验收完成")
    if begin > end:
        raise ValueError("来源验收登录日志时间顺序不成立")
    tenants = lineage.get("tenants")
    if not isinstance(tenants, list) or len(tenants) != 11 or len(subjects) != 11:
        raise ValueError("来源验收登录日志缺少十一租户身份")
    expected = {
        tenant["tenant_id"]: {
            "user_name": tenant["username"],
            "ipaddr": f"198.18.20.{index + 1}",
            "subject": subject,
        }
        for index, (tenant, subject) in enumerate(zip(tenants, subjects, strict=True))
    }
    if len(expected) != 11:
        raise ValueError("来源验收登录日志租户身份重复")
    observed, previous_id = set(), upper_id
    for row in new_rows:
        exact(row, LOGIN_ROW_FIELDS, "来源验收登录日志")
        tenant = row.get("tenant_id")
        try:
            row_id = int(row.get("id", ""))
            logged = dt.datetime.fromisoformat(row.get("login_time", "") + "+00:00")
        except (TypeError, ValueError) as error:
            raise ValueError("来源验收登录日志主键或时间无效") from error
        item = expected.get(tenant)
        if (
            item is None
            or tenant in observed
            or str(row_id) != row["id"]
            or row_id <= previous_id
            or row["user_name"] != item["user_name"]
            or row["ipaddr"] != item["ipaddr"]
            or row["status"] != "1"
            or row["msg"] is not None
            or not begin <= logged <= end
            or item["subject"]["tenant_id"] != tenant
            or any(value is not None and not isinstance(value, str)
                   for value in (row["login_location"], row["browser"], row["os"]))
        ):
            raise ValueError("来源验收新增登录日志与本次十一主体不同")
        observed.add(tenant)
        previous_id = row_id
    if observed != set(expected):
        raise ValueError("来源验收新增登录日志未精确覆盖十一租户")
    return {"old_rows_unchanged": True, "upper_id": str(upper_id), "new_rows": copy.deepcopy(new_rows)}


def _table_map(database: dict, location: str) -> dict[str, dict]:
    rows = database["target"]["database"].get("tables")
    if not isinstance(rows, list):
        raise ValueError(f"{location}完整数据库像缺少业务表")
    result = {}
    for row in rows:
        exact(row, {"table", "rows", "sha256"}, f"{location}数据库表")
        if (
            not isinstance(row["table"], str)
            or not row["table"]
            or type(row["rows"]) is not int
            or row["rows"] < 0
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", row["sha256"]) is None
            or row["table"] in result
        ):
            raise ValueError(f"{location}数据库表身份或摘要无效")
        result[row["table"]] = row
    return result


def image_write_effects(before: dict, after: dict) -> dict:
    """允许真实登录日志 +11；其他四库、对象、schema、owner、Redis 必须零漂移。"""
    exact(before, {"databases", "schema", "objects", "owners", "redis", "storage"}, "来源前像")
    exact(after, set(before), "来源后像")
    for field in ("schema", "objects", "owners", "redis", "storage"):
        if before[field] != after[field]:
            raise ValueError(f"来源验收期间 {field} 发生未登记变化")
    if set(before["databases"]) != set(after["databases"]) or "shared-control" not in before["databases"]:
        raise ValueError("来源验收前后像没有同一完整四库集合")
    for key in before["databases"]:
        if key != "shared-control" and before["databases"][key] != after["databases"][key]:
            raise ValueError("来源验收改变了租户业务数据库")
    first = copy.deepcopy(before["databases"]["shared-control"])
    second = copy.deepcopy(after["databases"]["shared-control"])
    before_tables, after_tables = _table_map(first, "来源前像"), _table_map(second, "来源后像")
    if set(before_tables) != set(after_tables) or not set(SESSION_TABLE_DELTAS).issubset(before_tables):
        raise ValueError("来源验收控制库表集合或会话审计表不完整")
    preserved = first["target"].get("preserved_tables"), second["target"].get("preserved_tables")
    if preserved[0] != preserved[1] or any(
        row.get("table") in SESSION_TABLE_DELTAS
        for rows in preserved
        if isinstance(rows, list)
        for row in rows
        if isinstance(row, dict)
    ):
        raise ValueError("来源验收不能把会话审计写入伪装为保留表")
    for table, expected_delta in SESSION_TABLE_DELTAS.items():
        previous, current = before_tables.pop(table), after_tables.pop(table)
        if current["rows"] - previous["rows"] != expected_delta:
            raise ValueError(f"来源验收 {table} 行数变化与当前产品合同不同")
        if (expected_delta == 0 and current != previous) or (
            expected_delta > 0 and current["sha256"] == previous["sha256"]
        ):
            raise ValueError(f"来源验收 {table} 内容摘要变化不可信")
    if before_tables != after_tables:
        raise ValueError("来源验收改变了控制库中的其他表")
    first["target"]["database"]["tables"] = []
    second["target"]["database"]["tables"] = []
    if first != second:
        raise ValueError("来源验收改变了控制库身份、placement 或保留表")
    return {
        "api_requests": {"login": 11, "logout": 11},
        "database_rows": copy.deepcopy(SESSION_TABLE_DELTAS),
        "authorization_cache": {"created_and_removed_keys": 33},
        "business_mutations": 0,
        "object_mutations": 0,
    }

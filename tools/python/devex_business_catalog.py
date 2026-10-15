"""只读受管业务目录；不扫描数据库，不猜测未登记业务表。"""
from pathlib import Path
import hashlib
import json
import re
import tomllib

from devex_clone_capture import regular_file


def _owned(package: Path, relative: str) -> Path:
    path = regular_file(package / relative)
    if not path.resolve().is_relative_to(package.resolve()):
        raise ValueError("业务目录资产越过已登记 crate")
    ownership = tomllib.loads(regular_file(package / ".ryframe/generated.toml").read_text(encoding="utf-8"))
    entries = [item for item in ownership.get("entries", []) if item.get("path") == relative]
    if (len(entries) != 1 or entries[0].get("resource") != "__catalog__"
            or hashlib.sha256(path.read_bytes()).hexdigest() != entries[0].get("content_hash")):
        raise ValueError("业务目录不是当前正式生成入口的受管资产")
    return path


def business_tables(backend: Path) -> list[dict]:
    """严格核验显式 Workspace 成员的机器目录与资源描述，未启用表也不能被静默过滤。"""
    manifest = backend / "Cargo.toml"
    if not manifest.is_file():
        return []
    workspace = tomllib.loads(regular_file(manifest).read_text(encoding="utf-8"))
    result = []
    seen = set()
    for member in workspace.get("workspace", {}).get("members", []):
        package = backend / member
        if not package.resolve().is_relative_to(backend.resolve()):
            raise ValueError("Workspace 成员越过源码根目录")
        package_manifest = package / "Cargo.toml"
        if not package_manifest.is_file():
            continue
        metadata = tomllib.loads(regular_file(package_manifest).read_text(encoding="utf-8"))
        marker = metadata.get("package", {}).get("metadata", {}).get("ryframe", {})
        if marker.get("kind") != "business":
            continue
        catalog = package / "src/generated/tenant_catalog.json"
        if not catalog.exists():
            empty = _owned(package, "src/generated/mod.rs").read_text(encoding="utf-8")
            if "pub static RESOURCES: [ryframe_sdk::ResourceDescriptor; 0] = [];" not in empty:
                raise ValueError("非空业务模块缺少完整租户目录；必须正式重新生成")
            continue
        value = json.loads(_owned(package, "src/generated/tenant_catalog.json").read_text(encoding="utf-8"))
        resources = json.loads(_owned(package, "src/generated/resources.json").read_text(encoding="utf-8"))
        if (set(value) != {"format_version", "tables"} or value["format_version"] != 1
                or resources.get("format_version") != 1 or resources.get("module") != marker.get("module")
                or not isinstance(value["tables"], list) or not isinstance(resources.get("resources"), list)):
            raise ValueError("业务机器目录版本、模块归属或完整结构无效")
        expected = {item["table"] for item in resources["resources"] if item["database"] == "tenant"}
        if expected != {item.get("table") for item in value["tables"]}:
            raise ValueError("业务资源与租户表目录不同")
        for table in value["tables"]:
            validate_table(table)
            if table["table"] in seen:
                raise ValueError("多个业务模块重复声明租户表")
            seen.add(table["table"])
            result.append(table)
    return result


def validate_table(table: dict) -> None:
    fields = {"table", "copy_order", "tenant_column", "primary_key_cursor_columns", "checksum_columns",
              "column_types", "has_generated_columns", "foreign_key_dependencies", "foreign_keys", "schema_canonical"}
    if (not isinstance(table, dict) or set(table) != fields or not isinstance(table["table"], str)
            or re.fullmatch(r"biz_[a-z0-9_]+", table["table"]) is None
            or table["tenant_column"] != "tenant_id" or table["has_generated_columns"] is not False
            or type(table["copy_order"]) is not int or not 0 < table["copy_order"] <= 2**31 - 1
            or not isinstance(table["schema_canonical"], str) or not table["schema_canonical"].strip()):
        raise ValueError("业务表描述不完整或包含未支持结构")
    columns, types = table["checksum_columns"], table["column_types"]
    if (not isinstance(columns, list) or not isinstance(types, list) or len(columns) != len(types)
            or len(set(columns)) != len(columns) or "tenant_id" not in columns
            or any(not isinstance(column, str) or re.fullmatch(r"[a-z0-9_]+", column) is None for column in columns)
            or any(not isinstance(kind, str) or re.fullmatch(r"[a-z]+", kind) is None for kind in types)
            or not table["primary_key_cursor_columns"]
            or not set(table["primary_key_cursor_columns"]).issubset(columns)):
        raise ValueError("业务表完整列、类型或复制 cursor 无效")


def business_columns(backend: Path) -> dict:
    return {table["table"]: dict(zip(table["checksum_columns"],
                                      [kind.upper() for kind in table["column_types"]], strict=True))
            for table in business_tables(backend)}


def business_ledger_columns(backend: Path) -> dict:
    source = regular_file(backend / "crates/ryframe-sdk/src/migration/ledger.rs").read_text(encoding="utf-8")
    matches = re.findall(r'const DDL: &str = "CREATE TABLE ryframe_business_migration \(([^"\n]+)\) ENGINE=', source)
    if len(matches) != 1:
        raise ValueError("正式业务迁移账本 DDL 缺失或不唯一")
    columns = re.findall(r"(?:^|, )([a-z_]+) (VARCHAR|CHAR|TIMESTAMP|JSON)\b", matches[0])
    if len(columns) != 10 or len(dict(columns)) != 10:
        raise ValueError("业务迁移账本完整列结构变化，必须审查复制规则")
    return dict(columns)

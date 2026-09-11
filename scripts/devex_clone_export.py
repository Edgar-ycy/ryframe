"""按当前编译目录只读导出完整源表和对象，保留逻辑摘要与文件摘要的区别。"""
from __future__ import annotations

import datetime as dt
import os
from pathlib import Path
import subprocess

from devex_clone import read_json
from devex_clone_capture import write_json
from devex_clone_model import bound_file, collect, declared_tenants, digest, exact, schema_fingerprints, state, validate_relations
from devex_clone_rows import parse_schema, reject_physical, rows, schema_catalog, validate_state
from devex_clone_schedule import schedule_row_sha256
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import BUCKETS, plan_hash


def schema_models(backend: Path) -> tuple[dict, dict, dict, dict]:
    control, tenant = schema_catalog(backend)
    full_control = parse_schema((backend / "sql/ryframe_config.sql").read_text(encoding="utf-8"))
    full_tenant = parse_schema((backend / "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs").read_text(encoding="utf-8"))
    return control, tenant, full_control, full_tenant


def schema_snapshot(tools: ExternalTools, models: tuple, phase: str) -> list[dict]:
    _, _, control, tenant = models
    result = []
    for database in tools.plan["source"]["databases"]:
        declared = {**(control if database["kind"] == "combined" else {}), **tenant}
        # 迁移账本本身不导出；其已应用版本由正式 migrate verify 核验。
        expected_tables = set(declared) | {"seaql_tenant_data_migrations"}
        if database["kind"] == "combined":
            expected_tables.add("seaql_migrations")
        raw = tools.mysql(database, "SELECT TABLE_NAME, COLUMN_NAME, UPPER(DATA_TYPE) FROM information_schema.COLUMNS "
                          "WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION;")
        actual = {}
        for line in raw.splitlines():
            parts = line.split("\t")
            if len(parts) != 3 or parts[1] in actual.setdefault(parts[0], {}):
                raise ValueError("源 schema 列清单无效或重复")
            actual[parts[0]][parts[1]] = parts[2]
        write_json(tools.work / f"schema-{phase}-{database['key']}.json", {"key": database["key"], "columns": actual})
        if set(actual) != expected_tables or any(list(actual[table].items()) != list(columns.items()) for table, columns in declared.items()):
            raise ValueError("源完整表或列与当前 schema catalog 不符，不能过滤未知表")
        result.append({"key": database["key"], "columns": actual, "sha256": plan_hash(actual)})
    return result


def run_cli(tools: ExternalTools, stage: str, command: list[str], backend: Path) -> None:
    path = tools.work / f"{stage}.command.json"
    write_json(path, {"command": command, "cwd": str(backend), "remote_operations": "read_only"})
    stdout, stderr, returncode, error_type = b"", b"", None, None
    try:
        result = tools.run(command, cwd=backend, env=dict(os.environ), check=True, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=1800,
                           creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        stdout, stderr, returncode = result.stdout, result.stderr, result.returncode
    except (OSError, subprocess.SubprocessError) as error:
        stdout, stderr = getattr(error, "stdout", stdout), getattr(error, "stderr", stderr)
        returncode, error_type = getattr(error, "returncode", None), type(error).__name__
        raise
    finally:
        write_json(tools.work / f"{stage}.diagnostic.json", {"returncode": returncode, "error_type": error_type,
                   "stdout": redact_object_diagnostic(stdout, os.environ), "stderr": redact_object_diagnostic(stderr, os.environ)})


def verify_migrations(tools: ExternalTools, backend: Path, maintenance: dict, stage: str) -> None:
    executable = cli_executable(maintenance, "migrate")
    run_cli(tools, f"{stage}-control", [executable, "control", "verify"], backend)
    for database in sorted(tools.plan["source"]["databases"], key=lambda item: item["key"]):
        cli_executable(maintenance, "migrate")
        run_cli(tools, f"{stage}-target-{database['key']}",
                [executable, "tenant-data", "verify", "--target", database["key"]], backend)


def cli_executable(maintenance: dict, role: str) -> str:
    artifact = maintenance["artifacts"][role]
    if file_digest(Path(artifact["executable"])) != {field: artifact[field] for field in ("bytes", "sha256")}:
        raise ValueError("维护 CLI 在执行前发生变化")
    return artifact["executable"]


def validate_inventory(value: dict, request: dict, models: tuple, source_sha: str, observation_at: str, *, observed: bool = False) -> None:
    time_field = "observed_at" if observed else "quiesced_at"
    exact(value, {"scope_id", "source_sha", time_field, "captured_at", "control_schema_fingerprint",
                  "tenant_schema_fingerprint", "databases", "objects"})
    if (value["scope_id"] != request["source"]["scope_id"] or value["source_sha"] != source_sha
            or dt.datetime.fromisoformat(value[time_field].replace("Z", "+00:00")) != dt.datetime.fromisoformat(observation_at)):
        raise ValueError("实际 inventory 与源 scope、构建或明确观察类型不同")
    captured = dt.datetime.fromisoformat(value["captured_at"].replace("Z", "+00:00"))
    if captured.tzinfo is None or not dt.datetime.fromisoformat(observation_at) <= captured <= dt.datetime.now(dt.timezone.utc):
        raise ValueError("实际 inventory 采集时间与观察顺序不符")
    schema_fingerprints(value)
    declared = {db["key"]: db for db in request["source"]["databases"]}
    databases = value["databases"]
    if (not isinstance(databases, list) or len(databases) != len(declared)
            or {db["key"] for db in databases} != set(declared)):
        raise ValueError("inventory 必须完整覆盖每个登记源目标，包括空目标；不能代填摘要")
    _, tenant, full_control, _ = models
    excluded = {"ryframe_resource_ownership", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
    for db in databases:
        exact(db, {"key", "kind", "server_uuid", "database", "shared", "placements", "tables"})
        known = declared[db["key"]]
        if any(db[field] != known[field] for field in ("kind", "server_uuid", "database")) or db["shared"] is not (known["mode"] == "shared"):
            raise ValueError("inventory 数据库 UUID、名称、类型或共享模式与源计划不同")
        expected = set(tenant) | (set(full_control) - excluded if known["kind"] == "combined" else set())
        if not isinstance(db["tables"], list) or len(db["tables"]) != len(expected) or {table["table"] for table in db["tables"]} != expected:
            raise ValueError("inventory 表清单不完整或包含未知表，空表也必须保留")
        for table in db["tables"]:
            exact(table, {"table", "rows", "sha256"})
            digest(table["sha256"])
            if type(table["rows"]) is not int or table["rows"] < 0:
                raise ValueError("inventory 表行数无效")
        tenants = set()
        for placement in db["placements"]:
            exact(placement, {"tenant_id", "generation", "switch_token"})
            if (not isinstance(placement["tenant_id"], str) or not placement["tenant_id"] or placement["tenant_id"] in tenants
                    or type(placement["generation"]) is not int or placement["generation"] <= 0
                    or not isinstance(placement["switch_token"], str) or not placement["switch_token"]):
                raise ValueError("inventory placement 重复或代次不完整")
            tenants.add(placement["tenant_id"])
    objects = value["objects"]
    if not isinstance(objects, list) or len(objects) != len(BUCKETS) or {item["bucket"] for item in objects} != BUCKETS:
        raise ValueError("inventory 必须完整覆盖五个固定桶")
    for bucket in objects:
        exact(bucket, {"bucket", "prefix", "entries"})
        if bucket["prefix"] != value["scope_id"] + "/" or not isinstance(bucket["entries"], list):
            raise ValueError("inventory 对象范围必须是源精确 scope 前缀")
        seen = set()
        for entry in bucket["entries"]:
            exact(entry, {"key", "bytes", "sha256"})
            digest(entry["sha256"])
            if (entry["key"] in seen or not entry["key"].startswith(bucket["prefix"])
                    or entry["key"] == bucket["prefix"] + ".ryframe-owner"
                    or type(entry["bytes"]) is not int or not 0 <= entry["bytes"] <= request["max_object_bytes"]):
                raise ValueError("inventory 对象重复、越界或超过明确大小上限")
            seen.add(entry["key"])


def capture_inventory(tools: ExternalTools, backend: Path, request: dict, generation: dict,
                      models: tuple, phase: str, observation_at: str, *, observed: bool = False) -> dict:
    filename = tools.work / f"inventory-{phase}.json"
    executable = cli_executable(generation["maintenance"], "tenant-data")
    sha = generation["source"]["head"]
    run_cli(tools, "inventory-" + phase, [executable, "backup-inventory", "--output", str(filename),
            "--source-sha", sha, "--observed-at" if observed else "--quiesced-at", observation_at], backend)
    value = read_json(filename)
    validate_inventory(value, request, models, sha, observation_at, observed=observed)
    return value


def logical_inventory(value: dict) -> dict:
    databases = [{**db, "tables": sorted(db["tables"], key=lambda row: row["table"]),
                  "placements": sorted(db["placements"], key=lambda row: row["tenant_id"])} for db in value["databases"]]
    objects = [{**bucket, "entries": sorted(bucket["entries"], key=lambda row: row["key"])} for bucket in value["objects"]]
    return {**{key: item for key, item in value.items() if key != "captured_at"},
            "databases": sorted(databases, key=lambda row: row["key"]), "objects": sorted(objects, key=lambda row: row["bucket"])}


def dump_databases(tools: ExternalTools, inventory: dict, models: tuple) -> list[dict]:
    control, tenant, _, _ = models
    directory = tools.work / "databases"
    directory.mkdir()
    outputs = []
    indexed = {db["key"]: db for db in inventory["databases"]}
    for database in sorted(tools.plan["source"]["databases"], key=lambda row: row["key"]):
        catalog = {**(control if database["kind"] == "combined" else {}), **tenant}
        path = directory / f"{database['key']}.sql"
        tools.dump(database, sorted(catalog), path)
        counts = {table: 0 for table in catalog}
        for table, _ in rows(path, catalog):
            counts[table] += 1
        expected = {table["table"]: table["rows"] for table in indexed[database["key"]]["tables"] if table["table"] in catalog}
        if counts != expected:
            raise ValueError("实际 SQL 转储不完整或行数与逻辑 inventory 不同")
        outputs.append({"key": database["key"], "tables": counts,
                        "artifact": {"file": path.relative_to(tools.work).as_posix(), **file_digest(path)}})
    return outputs


def inventory_object_index(inventory: dict) -> dict:
    """已校验 inventory 的声明摘要，仅用于提前发现 SQL 关系错误，不证明对象已抓取。"""
    return {(bucket["bucket"], item["key"].removeprefix(bucket["prefix"])):
            {field: item[field] for field in ("bytes", "sha256")}
            for bucket in inventory["objects"] for item in bucket["entries"]}


def validate_dump_state(tools: ExternalTools, models: tuple, dumps: list, objects: dict) -> list[dict]:
    """在同一遍完整解析中核对状态、关系输入及实际计数；扫描首尾重验绑定。"""
    source, observed = tools.plan["source"], state()
    control, tenant, _, _ = models
    indexed = {db["key"]: db for db in source["databases"]}
    logical_tenants = declared_tenants({"source": source, "databases": dumps}, tools.work, indexed, control)
    forbidden = (source["scope_id"], source["s3"]["endpoint"], *[db["database"] for db in source["databases"]])
    schedules = []
    for dump in dumps:
        exact(dump, {"key", "tables", "artifact"})
        catalog = {**(control if indexed[dump["key"]]["kind"] == "combined" else {}), **tenant}
        if (not isinstance(dump["tables"], dict) or set(dump["tables"]) != set(catalog)
                or any(type(count) is not int or count < 0 for count in dump["tables"].values())):
            raise ValueError("转储必须登记完整表集与非负整数行数，空表也不能省略")
        artifact = dict(dump["artifact"])
        declared_counts = dict(dump["tables"])
        path = bound_file(tools.work, artifact)
        counts = dict.fromkeys(catalog, 0)
        for table, row in rows(path, catalog):
            reject_physical(row, forbidden, logical_tenants)
            if table == "sys_job_schedule" and row["enabled"] == 1 and row["del_flag"] == "0":
                if row["tenant_id"] != "system":
                    raise ValueError("非 system 启用调度没有既定目标处置边界")
                schedules.append({"tenant_id": row["tenant_id"], "schedule_id": str(row["id"]),
                                  "handler_key": row["handler_key"], "expected_version": int(row["version"]),
                                  "source_row_sha256": schedule_row_sha256(row)})
            else:
                validate_state(table, row)
            counts[table] += 1
            collect(observed, table, row, dump["key"])
        if bound_file(tools.work, artifact) != path or dump["artifact"] != artifact:
            raise ValueError("转储文件或声明在状态扫描期间变化")
        if counts != declared_counts or dump["tables"] != declared_counts:
            raise ValueError("转储实际行数与完整登记表清单不同")
    validate_relations(observed, indexed, objects)
    return schedules

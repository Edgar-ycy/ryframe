"""以已绑定维护 CLI 只读采集四个明确目标；不创建资源，不提供复制写入入口。"""
from __future__ import annotations

import copy
from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import subprocess
from typing import Mapping
import uuid

from devex_clone_capture import read_json, regular_file, write_json
from devex_clone_export import cli_executable, schema_models
from devex_clone_model import digest, exact, linked, local_path, schema_fingerprints
from devex_clone_source_proof import validate_source
from devex_clone_tools import verify as verify_tools
from devex_clone_transfer import DatabaseObservation, PRESERVED
from restore_build import file_digest
from restore_reference_io import DatabaseVerificationError, ExternalTools, redact_object_diagnostic, verification_stdout
from restore_reference_plan import plan_hash
from restore_source_binding import defaults_connection, source_binding

KEYS = ("shared-control", "shared", "dedicated-a", "dedicated-b")


class InventoryCaptureError(RuntimeError):
    def __init__(self, directory: Path):
        self.evidence_directory = str(directory)
        super().__init__(f"四目标库存采集未通过；证据保留于 {directory}")


@dataclass(frozen=True)
class SideInventory:
    side: str
    binding: dict
    observations: tuple[DatabaseObservation, ...]
    receipt: dict
    receipt_file: dict


def expected_owners(scope: str, combined: bool) -> tuple[dict, ...]:
    return tuple({"resource_kind": kind, "scope_id": scope, "marker": f"ryframe-owner:v1:{scope}:{kind}"}
                 for kind in (("control", "tenant-data") if combined else ("tenant-data",)))


def table_digests(values: list) -> dict:
    if not isinstance(values, list):
        raise ValueError("库存逐表摘要必须为完整列表")
    result = {}
    for item in values:
        exact(item, {"table", "rows", "sha256"})
        if (not isinstance(item["table"], str) or item["table"] in result
                or type(item["rows"]) is not int or item["rows"] < 0):
            raise ValueError("库存表名重复或行数无效")
        result[item["table"]] = {"rows": item["rows"], "sha256": digest(item["sha256"])}
    return result


def observation_from_inventory(backend: Path, value: dict, declared: dict, scope: str,
                               ownership: tuple[dict, ...]) -> DatabaseObservation:
    """转换实际 CLI 结果及实际 owner 行；调用方仍须证明工具来源与当前连接配置。"""
    exact(value, {"scope_id", "control_schema_fingerprint", "tenant_schema_fingerprint", "target"})
    exact(value["target"], {"database", "preserved_tables"})
    database = value["target"]["database"]
    exact(database, {"key", "kind", "server_uuid", "database", "shared", "placements", "tables"})
    combined = declared["kind"] == "combined"
    if (value["scope_id"] != scope or any(database[field] != declared[field] for field in
            ("key", "kind", "server_uuid", "database")) or database["shared"] is not (declared["mode"] == "shared")
            or ownership != expected_owners(scope, combined)):
        raise ValueError("库存 scope、物理目标、共享模式或完整 owner 不匹配")
    control, tenant, full_control, _ = schema_models(backend)
    business = set(tenant) | (set(control) if combined else set())
    raw_preserved = {"ryframe_resource_ownership", "seaql_tenant_data_migrations"}
    if combined:
        raw_preserved |= {"seaql_migrations", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
    expected = business | raw_preserved | (set(full_control) if combined else set())
    tables = table_digests(database["tables"])
    kept = table_digests(value["target"]["preserved_tables"])
    if set(tables) & set(kept) or set(kept) != raw_preserved or set(tables) | set(kept) != expected:
        raise ValueError("库存完整表集、保留表或迁移账本不同，不能过滤未知表")
    all_tables = {**tables, **kept}
    preserved = set(all_tables) - business
    if not preserved.issubset(PRESERVED) or all_tables["ryframe_resource_ownership"]["rows"] != (2 if combined else 1):
        raise ValueError("库存存在未审查保留表或多余 ownership 行")
    placements = database["placements"]
    if not isinstance(placements, list):
        raise ValueError("库存缺少完整租户关系")
    seen = set()
    for item in placements:
        exact(item, {"tenant_id", "generation", "switch_token"})
        if (not isinstance(item["tenant_id"], str) or not item["tenant_id"] or item["tenant_id"] in seen
                or type(item["generation"]) is not int or item["generation"] <= 0
                or not isinstance(item["switch_token"], str) or not item["switch_token"]):
            raise ValueError("库存租户关系重复或代次无效")
        seen.add(item["tenant_id"])
    schema = schema_fingerprints(value)
    if not combined:
        schema["control_schema_fingerprint"] = None
    return DatabaseObservation(
        resource={"kind": "database", "scope_id": scope, "server_uuid": database["server_uuid"], "database": database["database"]},
        schema_sha256=plan_hash(schema), tables={name: all_tables[name] for name in sorted(business)},
        preserved={name: all_tables[name] for name in sorted(preserved)}, all_tables=tuple(sorted(all_tables)),
        ownership=copy.deepcopy(ownership))


def configuration(backend: Path, environment: Mapping[str, str], selected: dict) -> dict:
    directory = Path(environment.get("APP_CONFIG_DIR", str(backend / "config")))
    directory = directory if directory.is_absolute() else backend / directory
    if not directory.resolve().is_relative_to(backend.resolve()) or any(linked(p) for p in (directory, *directory.parents)):
        raise ValueError("库存配置必须位于明确后端内且不能经过链接")
    files = {}
    for path in sorted(directory.glob("*.toml")):
        if linked(path) or not path.is_file():
            raise ValueError("库存配置文件不能经过链接")
        files[path.name] = file_digest(path)
    if not files:
        raise ValueError("库存配置目录没有 TOML 文件")
    names = {name for name in environment if name.startswith("APP_")}
    credentials = {selected["s3"][field] for field in ("access_key_env", "secret_key_env")}
    names |= credentials
    if any(not environment.get(name) for name in credentials):
        raise ValueError("库存环境缺少已登记配置值")
    return {"directory": str(directory.resolve()), "files": files,
            "environment_sha256": plan_hash({name: environment[name] for name in sorted(names)})}


def capture_side_inventory(backend: Path, side: str, tools: ExternalTools, maintenance_receipt: Path,
                           output: Path, *, environment: Mapping[str, str]) -> SideInventory:
    """完整采集单侧四目标两次；成功只表示本次只读稳定，不证明停止历史或 fresh 资格。"""
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output), new=True)
    if not output.parent.is_dir():
        raise ValueError("库存输出须使用已有父目录中的新目录")
    output.mkdir()
    stage = "inputs"
    try:
        capture = _Capture(backend, side, tools, maintenance_receipt, output, environment)
        stage = "binding_before"
        before, maintenance = capture.binding()
        write_json(output / "binding-before.json", before)
        stage = "inventory_before"
        first = capture.observe("before", before, maintenance)
        stage = "inventory_after"
        second = capture.observe("after", before, maintenance)
        if first != second:
            raise ValueError("两次完整库存不同，不能声明只读稳定")
        stage = "binding_after"
        after, _ = capture.binding()
        if before != after:
            raise ValueError("采集期间来源、工具、配置或物理绑定变化")
        write_json(output / "binding-after.json", after)
        artifacts = {path.name: file_digest(regular_file(path)) for path in sorted(output.glob("*-target-*.json"))}
        if artifacts != capture.artifacts:
            raise ValueError("已采集库存文件缺失或被修改")
        receipt = {"format_version": 1, "status": "side_inventory_captured", "side": side,
                   "scope_id": capture.selected["scope_id"], "binding_sha256": plan_hash(before),
                   "keys": list(KEYS), "inventories": artifacts,
                   "observations": {key: asdict(value) for key, value in zip(KEYS, second[1], strict=True)},
                   "remote_writes": 0, "producer_stopped_proven": False, "fresh_target_proven": False,
                   "target_ready": False, "clone_verified": False, "restore_qualified": False}
        stage = "publish"
        write_json(output / "inventory.json", receipt)
        return SideInventory(side, before, second[1], receipt, {"path": str(output / "inventory.json"), **file_digest(output / "inventory.json")})
    except Exception as error:
        failure = {"format_version": 1, "status": "side_inventory_failed", "stage": stage,
                   "error_type": type(error).__name__, "remote_writes": 0, "target_ready": False, "clone_verified": False}
        if isinstance(error, DatabaseVerificationError):
            failure["database_verification"] = error.safe_details()
        write_json(output / "failure.json", failure)
        raise InventoryCaptureError(output) from None


class _Capture:
    def __init__(self, backend: Path, side: str, tools: ExternalTools, maintenance: Path,
                 output: Path, environment: Mapping[str, str]):
        self.backend, self.side, self.output, self.original = backend, side, output, tools
        self.original_environment, self.environment = environment, dict(environment)
        if any(not isinstance(key, str) or not isinstance(value, str) for key, value in self.environment.items()):
            raise ValueError("库存环境必须是明确文本映射")
        if side not in {"source", "target"} or side not in tools.plan:
            raise ValueError("库存必须选择已登记的 source 或 target")
        self.plan = copy.deepcopy(tools.plan)
        self.selected = self.plan[side]
        validate_source(backend, self.selected)
        declared = self.selected["databases"]
        if len(declared) != len(KEYS) or {item["key"] for item in declared} != set(KEYS):
            raise ValueError("库存必须完整覆盖登记的四个逻辑目标")
        self.databases = {item["key"]: item for item in declared}
        for key, item in self.databases.items():
            if item["mode"] != ("dedicated" if key.startswith("dedicated-") else "shared"):
                raise ValueError("四目标共享/独立模式与固定拓扑不同")
        self.maintenance = local_path(backend, str(maintenance))
        self.maintenance_digest = file_digest(self.maintenance)
        self.redaction = dict(self.environment)
        for key, item in self.databases.items():
            self.redaction["DATABASE_PASSWORD_" + key] = defaults_connection(backend, item)["password"]
        for field in ("access_key", "secret_key"):
            self.redaction[field.upper()] = self.environment.get(self.selected["s3"][field + "_env"], "")
        self.tools = ExternalTools(self.plan, output, self.run)
        self.artifacts = {}

    def run(self, command: list[str], **kwargs):
        # 外部 MySQL helper 仅提供禁止隐式 login-path 的值；其他环境均取本次显式快照。
        incoming = kwargs.pop("env", {}) or {}
        environment = {key: value for key, value in self.environment.items() if key != "MYSQL_PWD"}
        environment["MYSQL_TEST_LOGIN_FILE"] = str(self.output / "unused-login.cnf")
        if "MYSQL_TEST_LOGIN_FILE" in incoming and incoming["MYSQL_TEST_LOGIN_FILE"] != environment["MYSQL_TEST_LOGIN_FILE"]:
            raise ValueError("MySQL 隐式登录文件绑定改变")
        input_data = kwargs.get("input")
        input_digest = {"bytes": len(input_data), "sha256": hashlib.sha256(input_data).hexdigest()} if isinstance(input_data, bytes) else None
        stdout, stderr, code, error_type = b"", b"", None, None
        filename = self.output / ("command-" + uuid.uuid4().hex + ".json")
        with filename.open("x", encoding="utf-8", newline="\n") as stream:
            try:
                result = self.original.run(command, env=environment, **kwargs)
                stdout, stderr, code = result.stdout, result.stderr, result.returncode
                return result
            except (OSError, subprocess.SubprocessError) as error:
                stdout, stderr = getattr(error, "stdout", b""), getattr(error, "stderr", b"")
                code, error_type = getattr(error, "returncode", None), type(error).__name__
                raise
            finally:
                stdout, output_evidence = verification_stdout(kwargs.get("stdout"), stdout)
                json.dump({"command": command, "returncode": code, "error_type": error_type,
                           "stdin": input_digest,
                           "stdout": None if "stdout_capture_error" in output_evidence else redact_object_diagnostic(stdout, self.redaction),
                           **output_evidence,
                           "stderr": redact_object_diagnostic(stderr, self.redaction)}, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())

    def binding(self) -> tuple[dict, dict]:
        if self.original.plan != self.plan or dict(self.original_environment) != self.environment:
            raise ValueError("调用方计划或显式环境在采集期间变化")
        if file_digest(regular_file(self.maintenance)) != self.maintenance_digest:
            raise ValueError("维护工具收据变化")
        validate_source(self.backend, self.selected)
        physical = source_binding(self.backend, {"source": self.selected}, self.environment)
        config = configuration(self.backend, self.environment, self.selected)
        maintenance = verify_tools(self.backend, self.maintenance, self.run)
        self.tools.command("mysql")
        return {"side": self.side, "tools_plan_sha256": plan_hash(self.plan), "configuration": config,
                "physical": physical, "maintenance": self.maintenance_digest,
                "source": maintenance["source"]}, maintenance

    def observe(self, phase: str, expected: dict, maintenance: dict) -> tuple[dict, tuple[DatabaseObservation, ...]]:
        self.tools.verify_databases(self.side)
        raw, observations = {}, []
        schemas = None
        for key in KEYS:
            if self.binding()[0] != expected:
                raise ValueError("调用库存 CLI 前来源或配置已变化")
            executable = cli_executable(maintenance, "tenant-data")
            path = self.output / f"{phase}-target-{key}.json"
            self.run([executable, "target-inventory", "--target", key, "--output", str(path)],
                     cwd=self.backend, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1800,
                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            value = read_json(path)
            owners = self.tools.database_verification_response(self.databases[key], "ownership")
            rows = [line.split("\t") for line in owners.splitlines()]
            if any(len(row) != 3 for row in rows):
                raise DatabaseVerificationError("ownership", key, 2 if self.databases[key]["kind"] == "combined" else 1, owners)
            ownership = tuple(dict(zip(("resource_kind", "scope_id", "marker"), row, strict=True)) for row in rows)
            expected_ownership = expected_owners(self.selected["scope_id"], self.databases[key]["kind"] == "combined")
            if ownership != expected_ownership:
                raise DatabaseVerificationError("ownership", key, len(expected_ownership), owners)
            observed = observation_from_inventory(self.backend, value, self.databases[key], self.selected["scope_id"], ownership)
            fingerprints = (value["control_schema_fingerprint"], value["tenant_schema_fingerprint"])
            if schemas is not None and fingerprints != schemas:
                raise ValueError("四目标库存来自不同编译 schema")
            schemas = fingerprints
            raw[key] = value
            observations.append(observed)
            self.artifacts[path.name] = file_digest(regular_file(path))
        self.tools.verify_databases(self.side)
        return raw, tuple(observations)

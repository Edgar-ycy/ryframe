"""以已绑定维护 CLI 只读采集四个明确目标；不创建资源，不提供复制写入入口。"""
from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response, configuration as object_configuration

import copy
from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from typing import Mapping
import uuid

from artifact_digests import filesystem_path
from devex_clone_capture import read_json, regular_file, write_json
from devex_clone_export import cli_executable
from devex_clone_model import exact, linked, local_path
from devex_clone_source_proof import validate_source
from devex_clone_tools import verify as verify_tools
from devex_clone_transfer import DatabaseObservation
from restore_build import file_digest
from restore_reference_io import DatabaseVerificationError, ExternalTools, redact_object_diagnostic, verification_stdout
from restore_reference_plan import _native, plan_hash

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
    evidence_files: tuple[dict, ...]


def _entries(output: Path) -> list[os.DirEntry]:
    """深层验收目录可能超过 MAX_PATH，目录扫描统一走扩展路径。"""
    with os.scandir(filesystem_path(output)) as scan:
        return sorted(scan, key=lambda item: item.name)


def _evidence_files(output: Path, names: set[str]) -> tuple[dict, ...]:
    """绑定生产者实际声明的平面证据集，拒绝并发或特殊文件注入。"""
    entries = _entries(output)
    if ({entry.name for entry in entries} != names or len(entries) != len(names)
            or any(entry.is_symlink() or not entry.is_file(follow_symlinks=False) for entry in entries)
            or any(linked(output / entry.name) for entry in entries)):
        raise ValueError("库存目录包含生产者未声明的文件或目录")
    return tuple({"path": entry.name, "type": "file",
                  **file_digest(regular_file(output / entry.name))} for entry in entries)


def expected_owners(scope: str, combined: bool) -> tuple[dict, ...]:
    return tuple({"resource_kind": kind, "scope_id": scope, "marker": f"ryframe-owner:v1:{scope}:{kind}"}
                 for kind in (("control", "tenant-data") if combined else ("tenant-data",)))


def observation_from_inventory(backend: Path, value: dict, declared: dict, scope: str,
                               ownership: tuple[dict, ...]) -> DatabaseObservation:
    """转换实际 CLI 结果及实际 owner 行；调用方仍须证明工具来源与当前连接配置。"""
    result = _native("copy-database-observation", {
        "backend": str(backend), "value": value, "declared": declared,
        "scope": scope, "ownership": ownership,
    })
    result["all_tables"] = tuple(result["all_tables"])
    result["ownership"] = tuple(result["ownership"])
    return DatabaseObservation(**result)


def configuration(backend: Path, environment: Mapping[str, str], selected: dict, *,
                  evidence_root: Path | None = None, config_dir: Path | str | None = None) -> dict:
    if config_dir is None:
        directory = backend / "config"
    else:
        try:
            raw = os.fspath(config_dir)
        except (TypeError, ValueError, OSError):
            raise ValueError("库存配置目录必须为明确文本路径") from None
        if not isinstance(raw, str) or not raw or raw.strip() != raw:
            raise ValueError("库存配置目录必须为明确文本路径")
        directory = Path(raw)
        if any(ord(char) < 32 or 0x7F <= ord(char) <= 0x9F for char in raw) or ".." in directory.parts:
            raise ValueError("库存配置目录不能包含控制字符或父目录跳转")
        directory = directory if directory.is_absolute() else backend / directory
    if (not any(directory.resolve().is_relative_to(root.resolve()) for root in (backend, evidence_root or backend))
            or any(linked(p) for p in (directory, *directory.parents))):
        raise ValueError("库存配置必须位于明确后端内且不能经过链接")
    if not directory.is_dir():
        raise ValueError("库存配置目录必须存在")
    app_env = environment.get("APP_ENV")
    if not isinstance(app_env, str) or app_env not in ("dev", "test", "prod"):
        raise ValueError("库存 APP_ENV 必须明确选择 dev、test 或 prod")
    selected_file = directory / f"app.{app_env}.toml"
    if linked(selected_file) or not selected_file.is_file():
        raise ValueError("库存配置缺少无链接的已选环境文件")
    files = {}
    for path in sorted(directory.glob("*.toml")):
        if linked(path) or not path.is_file():
            raise ValueError("库存配置文件不能经过链接")
        files[path.name] = file_digest(path)
    if not files:
        raise ValueError("库存配置目录没有 TOML 文件")
    return {"directory": str(directory.resolve()), "files": files,
            "environment_sha256": plan_hash({"APP_ENV": app_env})}


def capture_side_inventory(backend: Path, side: str, tools: ExternalTools, maintenance_receipt: Path,
                           output: Path, *, environment: Mapping[str, str], evidence_root: Path | None = None,
                           control_environment=None) -> SideInventory:
    """完整采集单侧四目标两次；成功只表示本次只读稳定，不证明停止历史或 fresh 资格。"""
    backend = backend.resolve(strict=True)
    output = local_path(evidence_root or backend, str(output), new=True)
    if not output.parent.is_dir():
        raise ValueError("库存输出须使用已有父目录中的新目录")
    output.mkdir()
    stage = "inputs"
    published = None
    try:
        capture = _Capture(backend, side, tools, maintenance_receipt, output, environment,
                           evidence_root=evidence_root, control_environment=control_environment)
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
        artifacts = {entry.name: file_digest(regular_file(output / entry.name))
                     for entry in _entries(output) if re.fullmatch(r".*-target-.*\.json", entry.name)}
        if artifacts != capture.artifacts:
            raise ValueError("已采集库存文件缺失或被修改")
        receipt = {"format_version": 1, "status": "side_inventory_captured", "side": side,
                   "scope_id": capture.selected["scope_id"], "binding_sha256": plan_hash(before),
                   "keys": list(KEYS), "inventories": artifacts,
                   "observations": {key: asdict(value) for key, value in zip(KEYS, second[1], strict=True)},
                   "remote_writes": 0, "producer_stopped_proven": False, "fresh_target_proven": False,
                   "target_ready": False, "clone_verified": False, "restore_qualified": False}
        stage = "publish"
        fixed = {"binding-before.json", "binding-after.json"}
        _evidence_files(output, fixed | capture.evidence_names)
        inventory_path = output / "inventory.json"
        write_json(inventory_path, receipt)
        published = file_digest(inventory_path)
        evidence = _evidence_files(
            output, fixed | {"inventory.json"} | capture.evidence_names
        )
        return SideInventory(side, before, second[1], receipt,
                             {"path": str(inventory_path), **published}, evidence)
    except Exception as error:
        inventory_path = output / "inventory.json"
        if (published is not None and os.path.isfile(filesystem_path(inventory_path)) and not linked(inventory_path)
                and file_digest(inventory_path) == published):
            os.unlink(filesystem_path(inventory_path))
        failure = {"format_version": 1, "status": "side_inventory_failed", "stage": stage,
                   "error_type": type(error).__name__, "remote_writes": 0, "target_ready": False, "clone_verified": False}
        if isinstance(error, DatabaseVerificationError):
            failure["database_verification"] = error.safe_details()
        write_json(output / "failure.json", failure)
        raise InventoryCaptureError(output) from None


def verify_side_inventory(backend: Path, descriptor: dict, selected: dict, execution: Path) -> dict:
    """只读重算已采集的四目标完整像，不将保存证据表述为当前停机或 fresh 资格。"""
    from devex_clone_source_proof import bound_file

    path = bound_file(backend, descriptor)
    value = read_json(path)
    exact(value, {"format_version", "status", "side", "scope_id", "binding_sha256", "keys", "inventories", "observations",
                  "remote_writes", "producer_stopped_proven", "fresh_target_proven", "target_ready", "clone_verified", "restore_qualified"})
    if (path.name != "inventory.json" or os.path.exists(filesystem_path(path.parent / "failure.json"))
            or value["format_version"] != 1 or value["status"] != "side_inventory_captured"
            or value["side"] not in {"source", "target"} or value["scope_id"] != selected["scope_id"]
            or value["keys"] != list(KEYS) or value["remote_writes"] != 0
            or any(value[key] is not False for key in ("producer_stopped_proven", "fresh_target_proven", "target_ready", "clone_verified", "restore_qualified"))):
        raise ValueError("四目标完整像收据类型、scope 或资格字段不同")
    before = read_json(regular_file(path.parent / "binding-before.json"))
    if before != read_json(regular_file(path.parent / "binding-after.json")) or plan_hash(before) != value["binding_sha256"]:
        raise ValueError("四目标完整像来源前后绑定不同")
    names = {f"{phase}-target-{key}.json" for phase in ("before", "after") for key in KEYS}
    if set(value["inventories"]) != names or set(value["observations"]) != set(KEYS):
        raise ValueError("四目标完整像缺少明确的八份原始输出")
    observed, raw = {}, {}
    for db in selected["databases"]:
        key = db["key"]
        phases = []
        for phase in ("before", "after"):
            filename = f"{phase}-target-{key}.json"
            current = regular_file(path.parent / filename)
            if file_digest(current) != value["inventories"][filename]:
                raise ValueError("完整数据库像原始输出摘要变化")
            phases.append(read_json(current))
        if phases[0] != phases[1]:
            raise ValueError("完整数据库像采集期间变化")
        item = observation_from_inventory(execution, phases[0], db, selected["scope_id"], expected_owners(selected["scope_id"], db["kind"] == "combined"))
        observed[key] = json.loads(json.dumps(asdict(item)))
        raw[key] = phases[0]
    if observed != value["observations"] or bound_file(backend, descriptor) != path:
        raise ValueError("四目标逐表、保留表、placement 或 ownership 与原始输出不同")
    return {"receipt": value, "binding": before, "databases": raw}


class _Capture:
    def __init__(self, backend: Path, side: str, tools: ExternalTools, maintenance: Path,
                 output: Path, environment: Mapping[str, str], *, evidence_root: Path | None = None,
                 control_environment=None):
        self.backend, self.side, self.output, self.original = backend, side, output, tools
        self.control_environment = control_environment
        self.evidence_root = evidence_root or backend
        self.original_environment, self.environment = environment, dict(environment)
        if any(not isinstance(key, str) or not isinstance(value, str) for key, value in self.environment.items()):
            raise ValueError("库存环境必须是明确文本映射")
        if side not in {"source", "target"} or side not in tools.plan:
            raise ValueError("库存必须选择已登记的 source 或 target")
        self.plan = copy.deepcopy(tools.plan)
        self.selected = self.plan[side]
        validate_source(self.evidence_root, self.selected)
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
        self.database_credentials = {"backend": str(self.evidence_root), "databases": copy.deepcopy(list(self.databases.values()))}
        redact_object_diagnostic("", {}, database_credentials=self.database_credentials)
        objects = object_configuration(self.selected["s3"]["configuration"], self.evidence_root)
        for field in ("access_key", "secret_key"):
            self.redaction[field.upper()] = objects[field]
        self.evidence_names = set()
        self.tools = ExternalTools(self.plan, output, self.run, evidence_sink=self._track)
        self.artifacts = {}

    def _track(self, path: Path) -> None:
        if path.parent != self.output or Path(path.name).name != path.name:
            raise ValueError("库存生产者证据文件越出固定采集目录")
        self.evidence_names.add(path.name)

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
        self._track(filename)
        # 深层验收目录可能超过 MAX_PATH；收据写入统一走扩展路径。
        with open(filesystem_path(filename), "x", encoding="utf-8", newline="\n") as stream:
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
                if "stdout_file" in output_evidence:
                    self._track(Path(output_evidence["stdout_file"]["path"]))
                json.dump({"command": command, "returncode": code, "error_type": error_type,
                           "stdin": input_digest,
                           "stdout": None if "stdout_capture_error" in output_evidence else redact_object_diagnostic(stdout, self.redaction, database_credentials=self.database_credentials),
                           **output_evidence,
                           "stderr": redact_object_diagnostic(stderr, self.redaction, database_credentials=self.database_credentials)}, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())

    def binding(self) -> tuple[dict, dict]:
        if self.original.plan != self.plan or dict(self.original_environment) != self.environment:
            raise ValueError("调用方计划或显式环境在采集期间变化")
        if file_digest(regular_file(self.maintenance)) != self.maintenance_digest:
            raise ValueError("维护工具收据变化")
        validate_source(self.evidence_root, self.selected)
        physical = native_response(*native_invoke(
            "__source-evidence", {"operation": "physical-binding", "write": False,
                "arguments": {
                    'backend': str(self.backend),
                    'evidence_root': str(self.evidence_root),
                    'source': self.selected,
                    'app_env': self.environment.get("APP_ENV"),
                    'config_dir': None,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
        config = configuration(self.backend, self.environment, self.selected, evidence_root=self.evidence_root)
        if self.control_environment is None:
            maintenance = verify_tools(self.backend, self.maintenance, self.run)
        else:
            with self.control_environment():
                maintenance = verify_tools(self.backend, self.maintenance, self.original.run)
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
            self._track(path)
        self.tools.verify_databases(self.side)
        return raw, tuple(observations)

"""以已绑定维护 CLI 只读采集四个明确目标；不创建资源，不提供复制写入入口。"""
from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response

import copy
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import subprocess
from typing import Mapping

from artifact_digests import filesystem_path
from devex_clone_capture import read_json, regular_file, write_json
from devex_clone_model import exact, linked, local_path
from devex_clone_transfer import DatabaseObservation
from process_environment import configured
from restore_build import file_digest
from restore_reference_io import ExternalTools
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
                           output: Path, *, environment: Mapping[str, str], evidence_root: Path | None = None) -> SideInventory:
    """原生生产完整只读像；此端口只冻结输入、传输和还原既有类型。"""
    backend = backend.resolve(strict=True)
    root = (evidence_root or backend).resolve(strict=True)
    output = local_path(root, str(output), new=True)
    plan, private = copy.deepcopy(tools.plan), dict(environment)
    published = None
    try:
        maintenance = {"path": str(maintenance_receipt), **file_digest(regular_file(maintenance_receipt))}
        request = {"operation": "copy-side-inventory-capture", "write": True, "arguments": {
            "evidence_root": str(root), "backend": str(backend), "side": side, "plan": plan,
            "maintenance": maintenance, "configuration": plan.get(side, {}).get("s3", {}).get("configuration"),
            "app_env": private.get("APP_ENV"), "output": str(output)}}
        # 来源与工具链由Rust Scope重新观察；端口不切换父进程的控制/服务环境。
        value = native_response(*native_invoke(
            "__source-evidence", request, _inventory_transport, may_write=False))[0]["value"]
        exact(value, {"side", "binding", "receipt", "receipt_file", "evidence_files"})
        published = value["receipt_file"]
        if (tools.plan != plan or dict(environment) != private
                or file_digest(regular_file(maintenance_receipt)) != {key: maintenance[key] for key in ("bytes", "sha256")}):
            raise ValueError("调用方计划、环境或维护收据在采集期间变化")
        receipt, before = value["receipt"], value["binding"]
        path = output / "inventory.json"
        if (value["side"] != side or receipt["side"] != side or before["side"] != side
                or receipt["remote_writes"] != 0 or receipt["keys"] != list(KEYS)
                or before["tools_plan_sha256"] != plan_hash(plan)
                or published != {"path": str(path), **file_digest(regular_file(path))}
                or read_json(path) != receipt):
            raise ValueError("原生库存响应与原输入、发布文件或零写入声明不同")
        files = _evidence_files(output, {item["path"] for item in value["evidence_files"]})
        if list(files) != value["evidence_files"]:
            raise ValueError("原生库存完整证据集已变化")
        observations = []
        for key in KEYS:
            item = dict(receipt["observations"][key])
            item["all_tables"], item["ownership"] = tuple(item["all_tables"]), tuple(item["ownership"])
            observations.append(DatabaseObservation(**item))
        return SideInventory(side, before, tuple(observations), receipt, published, files)
    except Exception:
        # 只撤销本次精确发布像；未知文件/历史及远端资源均保留，不重放采集。
        path = output / "inventory.json"
        if (published is not None and not linked(path) and path.is_file()
                and published == {"path": str(path), **file_digest(path)}):
            os.unlink(filesystem_path(path))
        if not output.exists():
            output.mkdir()
        if output.is_dir() and not linked(output) and not (output / "failure.json").exists():
            write_json(output / "failure.json", {"format_version": 1, "status": "side_inventory_failed",
                "stage": "port_inputs_or_response", "error_type": "InventoryPortError", "remote_writes": 0,
                "target_ready": False, "clone_verified": False})
        raise InventoryCaptureError(output) from None


def _inventory_transport(command, **kwargs):
    # 旧采集没有整体期限；每条读命令仍由Rust保持原1800秒，不能套用通用端口整体期限。
    kwargs.pop("timeout", None)
    environment = configured({})
    environment.update({key: value for key, value in kwargs["env"].items()
                        if key.upper().startswith(("CARGO_", "RUSTUP_"))})
    kwargs["env"] = environment
    return subprocess.run(command, **kwargs)

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

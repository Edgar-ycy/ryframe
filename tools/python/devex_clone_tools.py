"""只读核验开发复制维护工具的来源、Cargo 事件与冻结产物。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from devex_clone import read_json
from devex_clone_model import exact, local_path
from devex_provenance import worktree_fingerprint
from restore_build import file_digest, source_snapshot
from source_fingerprints import checked_source, reusable_artifact_source
from source_inventory import _native as native_source

TOOLS = {"reset": ("bin-reset", "ryframe-reset"), "migrate": ("bin-migrate", "ryframe-migrate"),
         "tenant-data": ("bin-tenant-data", "ryframe-tenant-data")}


def command(role: str, target: Path) -> list[str]:
    feature, name = TOOLS[role]
    return ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features", "--features", feature,
            "--bin", name, "--jobs", "4", "--target-dir", str(target), "--message-format=json"]


def source_binding(backend: Path) -> dict:
    current = checked_source(backend)
    if current is not None:
        return {key: current[key] for key in ("snapshot", "worktree_fingerprint")}
    source = source_snapshot(backend)
    return {"snapshot": source, "worktree_fingerprint": worktree_fingerprint(backend, source["head"])}


def toolchain(backend: Path, run=subprocess.run) -> dict:
    result = {}
    for name, args in (("rustc", ["rustc", "-Vv"]), ("cargo", ["cargo", "-V"])):
        output = run(args, cwd=backend, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        result[name] = output.stdout.decode("utf-8").strip()
    return result


def target_directory(backend: Path) -> Path:
    return Path(native_source("maintenance-target-directory", root=str(backend)))


def recorded_cargo_path(backend: Path, name: str, content: str) -> Path:
    """读取原始构建事件；验证冻结副本不依赖可随时重建的缓存文件仍存在。"""
    found = []
    manifest = backend / "crates/ryframe/Cargo.toml"
    for line in content.splitlines():
        event = json.loads(line)
        if (event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == name
                and event.get("target", {}).get("kind") == ["bin"]
                and Path(event.get("manifest_path", "")).resolve() == manifest.resolve()
                and event.get("executable")):
            path = Path(event["executable"])
            if not path.is_absolute():
                raise ValueError("Cargo 产物事件必须使用实际绝对路径")
            found.append(path.resolve())
    if len(found) != 1:
        raise ValueError("维护工具必须恰好绑定一个当前 Workspace 的 Cargo 产物事件")
    return found[0]


def verify_evidence(backend: Path, filename: Path, receipt: dict | None = None) -> dict:
    """只读核验固定维护构建收据、Cargo 事件及冻结二进制，不比较当前源码。"""
    filename = local_path(backend, str(filename))
    if filename.name != "build.json" or (filename.parent / "failed.json").exists():
        raise ValueError("维护工具构建失败或收据名称不匹配")
    before = file_digest(filename)
    observed = read_json(filename)
    if receipt is not None and observed != receipt:
        raise ValueError("维护工具构建收据与调用方绑定内容不同")
    receipt = observed
    exact(receipt, {"format_version", "kind", "backend_root", "source", "toolchain", "target_directory",
                    "artifacts", "restore_qualified", "resources_modified"}
          | ({"source_inventory"} if "source_inventory" in receipt else set()))
    target = target_directory(backend)
    if (receipt["format_version"] != 1 or receipt["kind"] != "devex-clone-tool-build"
            or receipt["backend_root"] != str(backend) or receipt["target_directory"] != str(target)
            or receipt["restore_qualified"] is not False or receipt["resources_modified"] is not False
            or set(receipt["artifacts"]) != set(TOOLS)):
        raise ValueError("维护工具构建证据格式或用途不符")
    for role, item in receipt["artifacts"].items():
        exact(item, {"executable", "cargo_executable", "command", "cargo_output", "cargo_log", "bytes", "sha256"})
        executable = local_path(backend, item["executable"])
        if executable.parent != filename.parent or item["command"] != command(role, target):
            raise ValueError("维护工具路径或定向构建命令不匹配")
        if file_digest(executable) != {key: item[key] for key in ("bytes", "sha256")}:
            raise ValueError("维护工具实际二进制与收据不一致")
        for field in ("cargo_output", "cargo_log"):
            binding = item[field]
            exact(binding, {"file", "bytes", "sha256"})
            if binding["file"] != f"{role}.cargo." + ("jsonl" if field == "cargo_output" else "log"):
                raise ValueError("维护工具日志路径与角色不匹配")
            log = local_path(backend, str(filename.parent / binding["file"]))
            if file_digest(log) != {key: binding[key] for key in ("bytes", "sha256")}:
                raise ValueError("维护工具构建日志发生变化")
        # 校验原始事件的归属和准确路径；缓存可能重建，因此不要求缓存副本仍与冻结工具相同。
        output = filename.parent / item["cargo_output"]["file"]
        found = recorded_cargo_path(backend, TOOLS[role][1], output.read_text(encoding="utf-8"))
        if found != Path(item["cargo_executable"]) or not found.is_relative_to(target.resolve()):
            raise ValueError("维护工具 Cargo 事件与实际构建路径不一致")
    if file_digest(filename) != before or read_json(filename) != receipt:
        raise ValueError("校验维护工具期间收据发生变化")
    return receipt


def verify(backend: Path, filename: Path, run=subprocess.run) -> dict:
    """只读重新校验当前开发源码、工具链、Cargo 事件及冻结的维护 CLI 字节。"""
    filename = local_path(backend, str(filename))
    before = file_digest(filename)
    receipt = verify_evidence(backend, filename)
    current = source_binding(backend)
    original = reusable_artifact_source(backend, receipt)
    if (receipt["source"] != (original or current)
            or receipt["toolchain"] != toolchain(backend, run)):
        raise ValueError("维护工具未绑定当前完整开发源码和工具链")
    if file_digest(filename) != before or source_binding(backend) != current:
        raise ValueError("校验维护工具期间源码或收据变化")
    return receipt

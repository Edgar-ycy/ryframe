"""使用当前受控工具为任意登记的干净前端来源生成或核验构建收据。"""

from __future__ import annotations

import os
import stat
import subprocess
from pathlib import Path

from restore_build import repository
from restore_runtime_evidence import (
    HEX_40,
    ArtifactSnapshot,
    JsonDocument,
    artifact_snapshot,
    directory,
    is_reparse,
    read_json_document,
    validate_frontend_receipt,
)
from source_inventory import build_source_domains, capture_inventory, frontend_environment_files

FRONTEND_RECEIPT = ".vite/restore-build.json"
FRONTEND_BUILD_TOOL_FILES = (
    "scripts/build-source-inventory.mjs",
    "scripts/build-source.mjs",
    "scripts/restore-build.mjs",
)


def frontend_snapshots(root: Path) -> tuple[list[dict], list[ArtifactSnapshot]]:
    dist = directory(root / "dist", "前端 dist")
    snapshots = []
    for current, names, filenames in os.walk(dist, followlinks=False):
        base = Path(current)
        for name in names:
            child = base / name
            metadata = child.lstat()
            if stat.S_ISLNK(metadata.st_mode) or is_reparse(metadata) or not stat.S_ISDIR(metadata.st_mode):
                raise ValueError("前端产物包含链接、重解析点或非普通目录")
        for name in filenames:
            path = base / name
            relative = path.relative_to(dist).as_posix()
            if relative != FRONTEND_RECEIPT:
                snapshots.append(artifact_snapshot(path))
    snapshots.sort(key=lambda item: item.path.relative_to(dist).as_posix())
    files = [
        {"path": item.path.relative_to(dist).as_posix(), "bytes": item.bytes, "sha256": item.sha256}
        for item in snapshots
    ]
    return files, snapshots


def frontend_files(root: Path) -> list[dict]:
    return frontend_snapshots(root)[0]


def frontend_build_receipt(root: Path) -> Path:
    return root / "dist" / FRONTEND_RECEIPT


def registered_frontend_inventory(root: Path, expected_head: str) -> dict:
    if not HEX_40.fullmatch(expected_head):
        raise ValueError("前端构建来源提交必须是完整小写 SHA")
    inventory = capture_inventory(root)
    snapshot = inventory["source"]["snapshot"]
    if snapshot["head"] != expected_head or not snapshot["clean"]:
        raise ValueError("恢复构建必须使用登记的精确干净前端源码")
    return inventory


def validate_frontend_build(root: Path, inventory: dict | None = None) -> tuple[dict, JsonDocument]:
    """核验当前前端源码、有效环境与完整 dist 仍匹配真实构建收据。"""
    inventory = capture_inventory(root) if inventory is None else inventory
    receipt = read_json_document(frontend_build_receipt(root))
    value = validate_frontend_receipt(receipt.value)
    files, snapshots = frontend_snapshots(root)
    if (
        value["sources"] != build_source_domains(inventory, "frontend")
        or value["build"]["environment_files"] != frontend_environment_files(root)
        or value["files"] != files
    ):
        raise ValueError("前端构建收据与登记源码、环境或生产文件不一致")
    receipt.assert_unchanged()
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    if capture_inventory(root) != inventory:
        raise ValueError("核验期间前端源码发生变化")
    return inventory, receipt


def validate_registered_frontend(root: Path, expected_head: str) -> tuple[dict, JsonDocument]:
    return validate_frontend_build(root, registered_frontend_inventory(root, expected_head))


def build_registered_frontend(
    tools_frontend: Path,
    source_frontend: Path,
    expected_head: str,
    run=subprocess.run,
) -> tuple[Path, JsonDocument, str]:
    tools = repository(tools_frontend, "前端构建工具源码")
    source = repository(source_frontend, "前端构建来源")
    tools_inventory = capture_inventory(tools)
    if not tools_inventory["source"]["snapshot"]["clean"]:
        raise ValueError("前端构建工具必须来自干净工作树")
    source_inventory = registered_frontend_inventory(source, expected_head)
    tool_files = [artifact_snapshot(tools / relative) for relative in FRONTEND_BUILD_TOOL_FILES]
    receipt_path = frontend_build_receipt(source)
    action = "verified"
    if not receipt_path.exists():
        command = [
            "node",
            str(tools / "scripts/restore-build.mjs"),
            "external-build",
            "--source-root",
            str(source),
            "--expected-head",
            expected_head,
            "--write",
        ]
        run(
            command,
            cwd=tools,
            check=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        action = "built"
    inventory, receipt = validate_registered_frontend(source, expected_head)
    if inventory != source_inventory or capture_inventory(tools) != tools_inventory:
        raise ValueError("前端构建期间登记源码或受控工具发生变化")
    for snapshot in tool_files:
        snapshot.assert_unchanged()
    return source, receipt, action

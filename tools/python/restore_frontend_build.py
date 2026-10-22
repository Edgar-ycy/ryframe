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
from source_inventory import build_source_domains, capture_inventory
from restore_frontend_configuration import verify_frontend_inputs
from restore_frontend_paths import ordinary

FRONTEND_RECEIPT = ".vite/restore-build.json"
FRONTEND_BUILD_TOOL_FILES = (
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "vite.config.ts",
    "scripts/build-source-inventory.mjs",
    "scripts/build-source.mjs",
    "scripts/build-context-schema.mjs",
    "scripts/application-config.mjs",
    "scripts/application-config-toml.mjs",
    "scripts/application-config-files.mjs",
    "scripts/application-config-paths.mjs",
    "scripts/frontend-tool-config.mjs",
    "scripts/build-identity.mjs",
    "scripts/resource-render/json.mjs",
    "src/shared/config/apiEndpoint.ts",
    "scripts/restore-build.mjs",
)


def frontend_snapshots(root: Path) -> tuple[list[dict], list[ArtifactSnapshot]]:
    root = ordinary(str(root), directory=True)
    dist = directory(root / "dist", "前端 dist")
    snapshots = []

    def unreadable(error):
        raise ValueError(f"前端 dist 遍历失败：IO code {error.errno}；请修正产物权限后重建") from None

    for current, names, filenames in os.walk(dist, followlinks=False, onerror=unreadable):
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
    snapshots.sort(key=lambda item: item.path.relative_to(dist).as_posix().encode("utf-8"))
    files = [
        {"path": item.path.relative_to(dist).as_posix(), "bytes": item.bytes, "sha256": item.sha256}
        for item in snapshots
    ]
    if not {"index.html", ".vite/manifest.json"} <= {item["path"] for item in files}:
        raise ValueError("前端生产构建缺少 index.html 或 .vite/manifest.json；请重新构建")
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
    root = ordinary(str(root), directory=True)
    inventory = capture_inventory(root) if inventory is None else inventory
    receipt = read_json_document(frontend_build_receipt(root))
    value = validate_frontend_receipt(receipt.value)
    inputs = verify_frontend_inputs(root, value["build"])
    identity = value["build"]["identity"]["frontend_commit"]
    source = inventory["source"]["snapshot"]
    if identity != "development" and (not source["clean"] or identity != source["head"]):
        raise ValueError("正式前端构建身份与精确干净源码不一致")
    files, snapshots = frontend_snapshots(root)
    if (
        value["sources"] != build_source_domains(inventory, "frontend")
        or value["files"] != files
    ):
        raise ValueError("前端构建收据与登记源码、环境或生产文件不一致")
    receipt.assert_unchanged()
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    inputs.assert_unchanged()
    if frontend_files(root) != files:
        raise ValueError("核验期间前端完整 dist 文件集合或内容发生漂移")
    if capture_inventory(root) != inventory:
        raise ValueError("核验期间前端源码发生变化")
    return inventory, receipt


def validate_registered_frontend(root: Path, expected_head: str) -> tuple[dict, JsonDocument]:
    inventory, receipt = validate_frontend_build(root, registered_frontend_inventory(root, expected_head))
    verify_frontend_inputs(root, receipt.value["build"], expected_head=expected_head)
    return inventory, receipt


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
    for relative in ("vite.config.ts", "scripts/application-config.mjs",
                     "scripts/frontend-tool-config.mjs", "scripts/build-identity.mjs"):
        try:
            ordinary(str(source) + os.sep + relative)
        except ValueError:
            raise ValueError("登记前端缺少当前 TOML/身份文件布局；历史 B0 必须先登记正式适配来源") from None
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

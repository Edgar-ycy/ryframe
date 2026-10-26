"""使用当前受控工具为任意登记的干净前端来源生成或核验构建收据。"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from restore_build import repository
from restore_runtime_evidence import (
    HEX_40,
    ArtifactSnapshot,
    JsonDocument,
    artifact_snapshot,
    read_json_document,
)
from source_inventory import capture_inventory
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
    from restore_frontend_native import request

    files = request("artifacts", root)
    snapshots = [artifact_snapshot(root / "dist" / item["path"]) for item in files]
    for item, snapshot in zip(files, snapshots, strict=True):
        if snapshot.bytes != item["bytes"] or snapshot.sha256 != item["sha256"]:
            raise ValueError("前端产物在原生快照与读取之间漂移；请稳定文件后重建")
        snapshot.assert_unchanged()
    if request("artifacts", root) != files:
        raise ValueError("前端完整 dist 文件集合或内容发生漂移；请重新构建")
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
    """原生校验返回快照；本接口同时保留现有收据的使用窗口末端复核。"""
    from restore_frontend_native import request

    document = read_json_document(frontend_build_receipt(root))
    proof = request("validate", root, inventory=inventory)
    if proof["receipt"] != document.value:
        raise ValueError("前端构建收据在核验期间漂移；请稳定输入后重新构建")
    document.assert_unchanged()
    return proof["inventory"], document


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

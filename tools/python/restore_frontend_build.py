"""使用当前受控工具为任意登记的干净前端来源生成或核验构建收据。"""

from __future__ import annotations

from pathlib import Path

from restore_runtime_evidence import (
    HEX_40,
    ArtifactSnapshot,
    JsonDocument,
    artifact_snapshot,
    read_json_document,
)
from source_inventory import capture_inventory
from restore_frontend_configuration import verify_frontend_inputs

FRONTEND_RECEIPT = ".vite/restore-build.json"



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

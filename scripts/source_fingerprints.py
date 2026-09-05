"""按产品、验收工具和文档分域绑定当前完整源码。"""

from __future__ import annotations

import copy
import re
from pathlib import Path

from source_inventory import capture_inventory, fingerprints


def execution_source(inventory: dict) -> dict:
    """把一次受保护的完整清单转换为可记录的执行来源。"""
    return {**inventory["source"], "fingerprints": fingerprints(inventory)}


def current_execution_source(root: Path) -> dict:
    """在同一采集窗口绑定完整源码及其产品、工具和文档分域。"""
    return execution_source(capture_inventory(root))


def build_source(receipt: dict) -> dict:
    if receipt.get("kind") == "devex-clone-tool-build":
        return receipt["source"]
    return {"snapshot": receipt["source"]}


def verify_inventory_source(inventory: dict, receipt: dict) -> None:
    source = inventory["source"]
    expected = build_source(receipt)
    if (
        source["snapshot"] != expected["snapshot"]
        or "worktree_fingerprint" in expected
        and source["worktree_fingerprint"] != expected["worktree_fingerprint"]
        or re.fullmatch(r"sha256:[a-f0-9]{64}", source["worktree_fingerprint"]) is None
    ):
        raise ValueError("原始文件清单没有绑定构建时的完整源码")
    fingerprints(inventory)


def reusable_artifact_source(root: Path, receipt: dict) -> dict | None:
    """工具变化时允许复用产品输入未变的产物，并保留原构建来源。"""
    inventory = receipt.get("source_inventory")
    if inventory is None:
        return None
    verify_inventory_source(inventory, receipt)
    current = current_execution_source(root)
    if fingerprints(inventory)["product"] != current["fingerprints"]["product"]:
        raise ValueError("产品构建输入已经变化，必须重新编译")
    return copy.deepcopy(inventory["source"])

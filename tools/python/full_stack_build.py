"""从 Cargo JSON 事件解析当前工作区的唯一二进制产物。"""

from __future__ import annotations

import json
from pathlib import Path


def cargo_artifact(
    root: Path,
    name: str,
    output: str,
) -> Path:
    """从一次 Cargo JSON 输出中取得唯一的当前工作区二进制。"""
    manifest = (root / "crates/ryframe/Cargo.toml").resolve()
    found = []
    for line in output.splitlines():
        event = json.loads(line)
        belongs_to_workspace = Path(event.get("manifest_path", "")).resolve() == manifest
        is_binary = event.get("target", {}).get("kind") == ["bin"]
        if (
            event.get("reason") == "compiler-artifact"
            and event.get("target", {}).get("name") == name
            and event.get("executable")
            and is_binary
            and belongs_to_workspace
        ):
            found.append(Path(event["executable"]).resolve(strict=True))
    if len(found) != 1:
        raise ValueError(f"Cargo 必须恰好返回一次当前 Workspace 的 {name} 产物")
    return found[0]

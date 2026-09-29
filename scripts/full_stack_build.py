"""统一构建全栈与恢复流程需要的 RyFrame 二进制产物。"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Callable


ROLE_SPECS = {
    "api": ("bin-api", "ryframe"),
    "worker": ("bin-worker", "ryframe-worker"),
    "reset": ("bin-reset", "ryframe-reset"),
    "migrate": ("bin-migrate", "ryframe-migrate"),
}


def cargo_artifact(
    root: Path,
    name: str,
    output: str,
    *,
    require_workspace_manifest: bool = True,
    require_binary_kind: bool = True,
) -> Path:
    """从一次 Cargo JSON 输出中取得唯一的当前工作区二进制。"""
    manifest = (root / "crates/ryframe/Cargo.toml").resolve()
    found = []
    for line in output.splitlines():
        event = json.loads(line)
        belongs_to_workspace = (
            Path(event.get("manifest_path", "")).resolve() == manifest
            if require_workspace_manifest
            else True
        )
        is_binary = event.get("target", {}).get("kind") == ["bin"] if require_binary_kind else True
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


def build_artifacts(
    root: Path,
    roles: tuple[str, ...],
    run: Callable[[list[str]], str],
    *,
    require_workspace_manifest: bool = True,
    require_binary_kind: bool = True,
) -> dict[str, dict[str, object]]:
    """按明确角色构建产物，不负责来源快照、收据写入或目标目录管理。"""
    if not roles or len(set(roles)) != len(roles) or not set(roles) <= set(ROLE_SPECS):
        raise ValueError("构建角色必须是非空且不重复的已登记角色")
    artifacts = {}
    for role in roles:
        feature, name = ROLE_SPECS[role]
        command = [
            "cargo",
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            feature,
            "--bin",
            name,
            "--message-format=json",
        ]
        artifacts[role] = {
            "name": name,
            "command": command,
            "executable": cargo_artifact(
                root,
                name,
                run(command),
                require_workspace_manifest=require_workspace_manifest,
                require_binary_kind=require_binary_kind,
            ),
        }
    return artifacts

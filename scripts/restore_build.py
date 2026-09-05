"""从本次 Cargo JSON 构建绑定源码快照与二进制摘要。"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

from artifact_digests import file_digest
from source_inventory import capture_inventory, git, source_snapshot

ROLES = {"api": ("bin-api", "ryframe"), "worker": ("bin-worker", "ryframe-worker")}


def write_new(path: Path, value: dict, root: Path) -> None:
    if not path.resolve().is_relative_to((root / ".local-tests").resolve()):
        raise ValueError("运行收据只能写入当前仓库忽略的 .local-tests")
    git(root, "check-ignore", path.relative_to(root).as_posix())
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def cargo_artifact(root: Path, name: str, output: str) -> Path:
    found = []
    manifest = (root / "crates/ryframe/Cargo.toml").resolve()
    for line in output.splitlines():
        event = json.loads(line)
        if (event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == name
                and event.get("executable") and event.get("target", {}).get("kind") == ["bin"]
                and Path(event.get("manifest_path", "")).resolve() == manifest):
            found.append(Path(event["executable"]).resolve(strict=True))
    if len(found) != 1:
        raise ValueError(f"Cargo 必须恰好返回一次当前 Workspace 的 {name} 产物")
    return found[0]


def build(root: Path, run=subprocess.run) -> dict:
    source = source_snapshot(root)
    inventory = capture_inventory(root, source)
    artifacts = {}
    for role, (feature, name) in ROLES.items():
        command = ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                   "--features", feature, "--bin", name, "--message-format=json"]
        result = run(command, cwd=root, stdout=subprocess.PIPE, text=True, encoding="utf-8",
                     check=True, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        executable = cargo_artifact(root, name, result.stdout)
        artifacts[role] = {"executable": str(executable), "command": command, **file_digest(executable)}
    if source_snapshot(root) != source:
        raise ValueError("构建期间源码发生变化，不能登记混合来源产物")
    return {"format_version": 1, "kind": "restore-backend-build", "source": source,
            "source_inventory": inventory, "artifacts": artifacts}


def verify_build(root: Path, receipt: dict, sha: str) -> None:
    if (receipt.get("format_version") != 1 or receipt.get("kind") != "restore-backend-build"
            or receipt.get("source", {}).get("head") != sha or not receipt.get("source", {}).get("clean")
            or receipt["source"] != source_snapshot(root) or set(receipt.get("artifacts", {})) != set(ROLES)):
        raise ValueError("正式恢复必须使用精确干净 SHA 的本次构建收据")
    verify_build_artifacts(receipt)


def verify_build_artifacts(receipt: dict) -> None:
    if (receipt.get("format_version") != 1 or receipt.get("kind") != "restore-backend-build"
            or set(receipt.get("artifacts", {})) != set(ROLES)):
        raise ValueError("构建收据缺少完整 API 与 Worker 产物")
    for role, artifact in receipt["artifacts"].items():
        feature, name = ROLES[role]
        if artifact.get("command") != ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                                        "--features", feature, "--bin", name, "--message-format=json"]:
            raise ValueError("恢复二进制构建命令不匹配")
        actual = file_digest(Path(artifact["executable"]))
        if any(artifact.get(key) != value for key, value in actual.items()):
            raise ValueError("恢复二进制文件与构建收据不一致")

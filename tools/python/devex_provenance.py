"""核验已有消费者所需的构建来源及工作树指纹。"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path

from restore_build import source_snapshot
from source_fingerprints import build_source, checked_source, reusable_artifact_source
from source_inventory import git


def worktree_fingerprint(root: Path, commit: str) -> str:
    # 与 xtask metadata::worktree_fingerprint 使用完全相同的 Git 参数及长度前缀。
    digest = hashlib.sha256()

    def update(value: bytes) -> None:
        digest.update(len(value).to_bytes(8, "little"))
        digest.update(value)

    update(commit.encode())
    update(git(root, "diff", "--binary", "--no-ext-diff", "HEAD", "--", "."))
    for raw in git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"):
        if not raw:
            continue
        relative = Path(raw.decode("utf-8"))
        path = root / relative
        if (
            relative.is_absolute()
            or ".." in relative.parts
            or path.is_symlink()
            or not path.resolve(strict=True).is_relative_to(root.resolve())
        ):
            raise ValueError("源码包含越界的未跟踪路径")
        update(raw)
        update(path.read_bytes())
    return "sha256:" + digest.hexdigest()


def verify_source(root: Path, receipt: dict, fingerprint: str) -> dict:
    original = reusable_artifact_source(root, receipt)
    if original is not None:
        if original["worktree_fingerprint"] != fingerprint:
            raise ValueError("请求没有绑定构建时的完整工作区指纹")
        return original["snapshot"]
    current = checked_source(root)
    source = current["snapshot"] if current is not None else source_snapshot(root)
    actual = (
        current["worktree_fingerprint"]
        if current is not None
        else worktree_fingerprint(root, source["head"])
    )
    expected = build_source(receipt)["snapshot"]
    if (
        expected != source
        or type(source.get("clean")) is not bool
        or not re.fullmatch(r"[a-f0-9]{40}", source["head"])
        or actual != fingerprint
    ):
        raise ValueError("实际源码与本侧精确构建快照或 DevEx 指纹不一致")
    return source

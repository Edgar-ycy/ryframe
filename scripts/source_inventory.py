"""只读采集 Git 快照、完整文件清单和来源分类，不依赖复制或恢复流程。"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import stat
import subprocess

from artifact_digests import file_digest


def git(root: Path, *arguments: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *arguments])


def snapshot(root: Path) -> tuple[dict, bytes]:
    head = git(root, "rev-parse", "HEAD").decode().strip()
    if not re.fullmatch(r"[a-f0-9]{40}", head):
        raise ValueError("源码 HEAD 不是有效 SHA")
    patch = git(root, "diff", "--binary", "HEAD")
    files = []
    for raw in sorted(git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")):
        if not raw:
            continue
        relative = raw.decode("utf-8")
        source_domain(relative)
        path = root / relative
        if path.is_symlink() or not path.resolve().is_relative_to(root) or not path.is_file():
            raise ValueError("未跟踪文件越界、不是普通文件或包含符号链接")
        files.append({"path": relative, "sha256": file_digest(path)["sha256"]})
    return {"head": head, "patch_sha256": hashlib.sha256(patch).hexdigest(), "files": files}, patch


def source_snapshot(root: Path) -> dict:
    if Path(git(root, "rev-parse", "--show-toplevel").decode().strip()).resolve() != root.resolve():
        raise ValueError("恢复构建必须使用实际 Git 仓库根目录")
    result = snapshot(root)[0]
    result["clean"] = not git(root, "status", "--porcelain", "--untracked-files=all").strip()
    return result


def worktree_fingerprint(root: Path, commit: str) -> str:
    """与 xtask 使用相同的 Git 参数、原始文件字节及八字节小端长度前缀。"""
    digest = hashlib.sha256()

    def update(value: bytes) -> None:
        digest.update(len(value).to_bytes(8, "little"))
        digest.update(value)

    update(commit.encode())
    update(git(root, "diff", "--binary", "--no-ext-diff", "HEAD", "--", "."))
    for raw in git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"):
        if not raw:
            continue
        source_domain(raw.decode("utf-8"))
        relative = Path(raw.decode("utf-8"))
        path = root / relative
        if (relative.is_absolute() or ".." in relative.parts or path.is_symlink()
                or not path.resolve(strict=True).is_relative_to(root.resolve())):
            raise ValueError("源码包含越界的未跟踪路径")
        update(raw)
        update(path.read_bytes())
    return "sha256:" + digest.hexdigest()


def canonical_digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True,
                                    separators=(",", ":")).encode()).hexdigest()


def source_domain(relative: str) -> str:
    path = PurePosixPath(relative)
    if (not relative or path.is_absolute() or PureWindowsPath(relative).drive
            or ".." in path.parts or "\\" in relative
            or path.as_posix() != relative):
        raise ValueError("源码清单路径无效")
    # 生成器嵌入的版本阶段实现属于产品构建输入。
    if relative == "scripts/release_stage.py":
        return "product"
    if (path.parts[0] == "tests" or path.parts[:2] == ("xtask", "tests")
            or len(path.parts) >= 3 and path.parts[0] == "crates" and path.parts[2] == "tests"):
        return "test_tools"
    if path.parts[0] == "scripts" and path.suffix in {".py", ".mjs", ".js", ".txt"}:
        return "test_tools"
    if path.parts[0] == ".github":
        return "test_tools"
    if path.parts[0] == "docs" or relative in {"README.md", "CHANGELOG.md", "LICENSE"}:
        return "support"
    return "product"


def fingerprints(inventory: dict) -> dict:
    groups = {key: [] for key in ("product", "test_tools", "support")}
    previous = ""
    for item in inventory["files"]:
        if (set(item) != {"path", "sha256"} or not re.fullmatch(r"[a-f0-9]{64}", item["sha256"])
                or item["path"] <= previous):
            raise ValueError("源码清单必须是无重复的排序文件摘要")
        previous = item["path"]
        groups[source_domain(item["path"])].append(item)
    return {key: {"sha256": canonical_digest(values), "files": len(values)} for key, values in groups.items()}


def file_inventory(root: Path) -> dict:
    resolved, files, modes = root.resolve(), [], []
    paths = git(root, "ls-files", "--cached", "--others", "--exclude-standard", "-z")
    for raw in sorted(set(paths.split(b"\0"))):
        if not raw:
            continue
        relative = raw.decode("utf-8")
        source_domain(relative)
        path = root / relative
        try:
            observed = path.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(observed.st_mode) or not path.resolve().is_relative_to(resolved):
            raise ValueError("源码清单包含链接或越界路径")
        if not stat.S_ISREG(observed.st_mode):
            raise ValueError("源码清单只能绑定普通文件")
        files.append({"path": relative, "sha256": file_digest(path)["sha256"]})
        modes.append({"path": relative, "executable": observed.st_mode & 0o111})
    return {"files": files, "head": git(root, "rev-parse", "HEAD").decode().strip(),
            "index_sha256": hashlib.sha256(git(root, "ls-files", "--stage", "-z")).hexdigest(),
            "modes_sha256": canonical_digest(modes)}


def capture_inventory(root: Path, source: dict | None = None) -> dict:
    before = source_snapshot(root) if source is None else source
    fingerprint = worktree_fingerprint(root, before["head"])
    guard = file_inventory(root)
    if (source_snapshot(root) != before
            or worktree_fingerprint(root, before["head"]) != fingerprint or guard["head"] != before["head"]):
        raise ValueError("采集指纹期间完整源码发生变化")
    return {"source": {"snapshot": before, "worktree_fingerprint": fingerprint}, "files": guard["files"],
            "guard": {key: value for key, value in guard.items() if key != "files"}}

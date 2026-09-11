"""绑定 B0/B1 构建与同一恢复导出使用的精确源码证据。"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess
import uuid

from restore_build import file_digest


B0_BACKEND_COMMIT = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6"
B0_FRONTEND_COMMIT = "0087ea2ecf62530d042b9e52f5c950fb34c66d78"
B0_ADAPTER_COMMIT = "c05114bcdf5c369cd74087db6317ce3c8f89bee8"
B0_ADAPTER_TREE = "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7"
B0_ADAPTER_PATCH = Path("xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch")
B0_ADAPTER_PATCH_SHA256 = "28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3"
B0_ADAPTER_PATHS = ["xtask/src/cli.rs"]


def _git(root: Path, *arguments: str, index: Path | None = None) -> bytes:
    environment = os.environ if index is None else {**os.environ, "GIT_INDEX_FILE": str(index)}
    result = subprocess.run(
        ["git", *arguments], cwd=root, env=environment, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, check=False,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    if result.returncode != 0:
        raise ValueError(f"无法核验 B0 适配 Git 对象：git {' '.join(arguments)}")
    return result.stdout


def _text(root: Path, *arguments: str, index: Path | None = None) -> str:
    try:
        return _git(root, *arguments, index=index).decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError:
        raise ValueError("B0 适配 Git 输出不是 UTF-8") from None


def _patch_bytes(root: Path) -> bytes:
    path = root / B0_ADAPTER_PATCH
    if not path.is_file() or path.is_symlink():
        raise ValueError("B0 内嵌适配补丁不是仓库内普通文件")
    return path.read_bytes()


def _reconstructed_tree(root: Path, patch: Path) -> str:
    scratch = root / "target"
    scratch.mkdir(exist_ok=True)
    index = scratch / f"b0-adapter-{os.getpid()}-{uuid.uuid4().hex}.index"
    lock = Path(str(index) + ".lock")
    try:
        _git(root, "read-tree", B0_BACKEND_COMMIT, index=index)
        _git(root, "apply", "--cached", "--whitespace=nowarn", str(patch), index=index)
        return _text(root, "write-tree", index=index)
    finally:
        for path in (lock, index):
            path.unlink(missing_ok=True)


def b0_adapter_evidence(root: Path) -> dict:
    """证明登记提交恰由内嵌工具补丁从原 B0 重建，且未越过工具层。"""
    root = root.resolve(strict=True)
    actual_root = Path(_text(root, "rev-parse", "--show-toplevel")).resolve(strict=True)
    if actual_root != root:
        raise ValueError("B0 适配核验必须从实际后端 Git 根目录执行")
    patch_path = root / B0_ADAPTER_PATCH
    patch = _patch_bytes(root)
    digest = hashlib.sha256(patch).hexdigest()
    reference = _git(root, "diff", "--binary", "--full-index", B0_BACKEND_COMMIT,
                     B0_ADAPTER_COMMIT, "--", ".")
    parent = _text(root, "rev-parse", f"{B0_ADAPTER_COMMIT}^")
    tree = _text(root, "rev-parse", f"{B0_ADAPTER_COMMIT}^{{tree}}")
    paths = [item.decode("utf-8", errors="strict") for item in _git(
        root, "diff", "--name-only", "-z", B0_BACKEND_COMMIT, B0_ADAPTER_COMMIT,
        "--", ".").split(b"\0") if item]
    rebuilt = _reconstructed_tree(root, patch_path)
    if (parent != B0_BACKEND_COMMIT or tree != B0_ADAPTER_TREE or paths != B0_ADAPTER_PATHS
            or digest != B0_ADAPTER_PATCH_SHA256 or reference != patch or rebuilt != tree):
        raise ValueError("B0 工具适配提交、补丁摘要、路径或重建树不匹配")
    return {
        "contract": "legacy-stable-readiness-b0-v1",
        "base_backend_sha": B0_BACKEND_COMMIT,
        "base_frontend_sha": B0_FRONTEND_COMMIT,
        "reference_adapter_sha": B0_ADAPTER_COMMIT,
        "adapter_tree": B0_ADAPTER_TREE,
        "adapter_paths": B0_ADAPTER_PATHS,
        "patch": {"path": B0_ADAPTER_PATCH.as_posix(), **file_digest(patch_path)},
    }

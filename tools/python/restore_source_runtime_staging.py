"""从已登记 Git 对象和原生工具绑定创建来源验收不可变快照。"""

from __future__ import annotations

from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess

from devex_clone_model import linked, local_path
from devex_clone_source_proof import bound_file
from restore_runtime_evidence import artifact_snapshot, exact_fields
from source_fingerprints import verify_execution_source


DIRECTORY = "source-tools"
MANIFEST = "manifest.json"
ENTRY = "__recovery-existing-source"
CONTRACT = "openapi/openapi.json"
MANIFEST_FIELDS = {
    "format_version", "kind", "coordinator_source", "execution_sha", "entry", "contract",
    "runtime_modules", "files",
}


def _git(root: Path, *arguments: str) -> bytes:
    environment = {
        key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")
    }
    environment.update({"GIT_OPTIONAL_LOCKS": "0", "GIT_NO_REPLACE_OBJECTS": "1"})
    return subprocess.check_output(
        ["git", "-C", str(root), *arguments], env=environment
    )


def _tree_files(root: Path, revision: str, prefix: str) -> list[str]:
    raw = _git(root, "ls-tree", "-r", "-z", revision, "--", prefix)
    result = []
    for entry in raw.split(b"\0"):
        if not entry:
            continue
        try:
            header, encoded = entry.split(b"\t", 1)
            mode, kind, _object = header.decode("ascii").split(" ")
            relative = encoded.decode("utf-8")
        except (ValueError, UnicodeDecodeError) as error:
            raise ValueError("来源验收 Git 树包含无效路径") from error
        path = PurePosixPath(relative)
        if (
            kind != "blob"
            or mode not in {"100644", "100755"}
            or path.is_absolute()
            or ".." in path.parts
            or path.as_posix() != relative
        ):
            raise ValueError("来源验收 Git 树包含非普通文件或无效路径")
        result.append(relative)
    return result


def _blob(root: Path, revision: str, relative: str) -> bytes:
    return _git(root, "cat-file", "blob", f"{revision}:{relative}")


@lru_cache(maxsize=16)
def _committed_sources(
    coordinator_name: str,
    execution_name: str,
    coordinator_sha: str,
    execution_sha: str,
):
    execution = Path(execution_name)
    if not re.fullmatch(r"[a-f0-9]{40}", coordinator_sha):
        raise ValueError("来源验收协调器提交无效")
    contract = _tree_files(execution, execution_sha, CONTRACT)
    if contract != [CONTRACT]:
        raise ValueError("来源验收执行快照缺少唯一 OpenAPI 契约")
    return _blob(execution, execution_sha, CONTRACT)


def _sources(coordinator: Path, execution: Path, coordinator_sha: str, execution_sha: str):
    contract = _committed_sources(
        str(coordinator.resolve(strict=True)), str(execution.resolve(strict=True)),
        coordinator_sha, execution_sha,
    )
    return [("execution", CONTRACT, contract)]


def _write_new(path: Path, content: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return artifact_snapshot(path).descriptor()


def _root(coordinator: Path, directory: Path, *, existing: bool) -> Path:
    coordinator = coordinator.resolve(strict=True)
    directory = local_path(coordinator, str(directory))
    if not directory.is_dir():
        raise ValueError("来源工具 staging 必须位于已存在的 verification 目录")
    root = directory / DIRECTORY
    if linked(root) or (existing and not root.is_dir()) or (not existing and root.exists()):
        raise ValueError("来源工具 staging 必须是唯一 create-only 普通目录")
    return root


def _create_directories(root: Path, rows: list[tuple[str, str, bytes]]) -> None:
    root.mkdir()
    created = {PurePosixPath(".")}
    parents = sorted(
        {parent for _origin, relative, _content in rows
         for parent in PurePosixPath(relative).parents if parent != PurePosixPath(".")},
        key=lambda path: (len(path.parts), path.as_posix()),
    )
    for relative in parents:
        parent = relative.parent
        if parent not in created or linked(root / parent):
            raise ValueError("来源工具 staging 目录父级不同或包含链接")
        (root / relative).mkdir()
        created.add(relative)


def create_tool_staging(
    coordinator: Path,
    execution: Path,
    directory: Path,
    coordinator_source: dict,
    execution_sha: str,
    tool: dict,
) -> dict:
    """从执行 commit 复制契约，并绑定协调器核验过的原生工具。"""
    verify_execution_source(coordinator_source, "来源验收协调器")
    coordinator_sha = coordinator_source["snapshot"]["head"]
    if (
        coordinator_source["snapshot"]["clean"] is not True
        or not isinstance(execution_sha, str)
        or re.fullmatch(r"[a-f0-9]{40}", execution_sha) is None
    ):
        raise ValueError("来源工具 staging 要求干净协调器与精确执行 SHA")
    root = _root(coordinator, directory, existing=False)
    rows = _sources(
        coordinator, execution.resolve(strict=True), coordinator_sha, execution_sha
    )
    _create_directories(root, rows)
    files = []
    for origin, relative, content in rows:
        descriptor = _write_new(root / PurePosixPath(relative), content)
        files.append({"path": str((root / PurePosixPath(relative)).resolve()), "origin": origin, "bytes": descriptor["bytes"],
                      "sha256": descriptor["sha256"]})
    from devex_clone_target_binding import external_file
    external_file(tool)
    executable = artifact_snapshot(Path(tool["path"]).resolve(strict=True)).descriptor()
    executable = {key: executable[key] for key in ("path", "bytes", "sha256")}
    manifest = {
        "format_version": 2,
        "kind": "restore-source-native-staging",
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "entry": ENTRY,
        "contract": str((root / CONTRACT).resolve()),
        "runtime_modules": [executable],
        "files": files,
    }
    raw = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    return _write_new(root / MANIFEST, raw)


def verify_tool_staging(
    coordinator: Path,
    execution: Path,
    directory: Path,
    descriptor: dict,
    coordinator_source: dict,
    execution_sha: str,
    tool: dict,
) -> dict:
    """重读 Git blob 和 staging 全集；不从可变 checkout 解析代码或契约。"""
    root = _root(coordinator, directory, existing=True)
    manifest_path = bound_file(coordinator, descriptor)
    if manifest_path != root / MANIFEST:
        raise ValueError("来源工具 staging 清单不属于同一 verification 目录")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("来源工具 staging 清单不是有效 UTF-8 JSON") from error
    exact_fields(manifest, MANIFEST_FIELDS, "来源工具 staging 清单")
    verify_execution_source(coordinator_source, "来源验收协调器")
    expected_rows = _sources(
        coordinator.resolve(strict=True), execution.resolve(strict=True),
        coordinator_source["snapshot"]["head"], execution_sha,
    )
    expected_files = []
    expected_paths = {MANIFEST}
    for origin, relative, content in expected_rows:
        path = root / PurePosixPath(relative)
        if linked(path) or not path.is_file():
            raise ValueError("来源工具 staging 包含链接、缺失或非普通文件")
        expected_paths.add(relative)
        expected_files.append({"path": str(path.resolve()), "origin": origin, "bytes": len(content),
                               "sha256": hashlib.sha256(content).hexdigest()})
        if path.read_bytes() != content:
            raise ValueError("来源工具 staging 与已登记 Git blob 不同")
    observed = set()
    for path in root.rglob("*"):
        if linked(path):
            raise ValueError("来源工具 staging 包含链接或重解析点")
        if path.is_file():
            observed.add(path.relative_to(root).as_posix())
        elif not path.is_dir():
            raise ValueError("来源工具 staging 包含非普通文件")
    if observed != expected_paths:
        raise ValueError("来源工具 staging 文件集合包含缺失或未知项")
    from devex_clone_target_binding import external_file
    external_file(tool)
    executable = artifact_snapshot(Path(tool["path"]).resolve(strict=True)).descriptor()
    executable = {key: executable[key] for key in ("path", "bytes", "sha256")}
    expected = {
        "format_version": 2,
        "kind": "restore-source-native-staging",
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "entry": ENTRY,
        "contract": str((root / CONTRACT).resolve()),
        "runtime_modules": [executable],
        "files": expected_files,
    }
    if (
        type(manifest["format_version"]) is not int
        or manifest != expected
        or artifact_snapshot(manifest_path).descriptor() != descriptor
    ):
        raise ValueError("来源工具 staging 清单、来源或摘要不同")
    return {"root": root, "entry": ENTRY, "contract": root / CONTRACT,
            "manifest": manifest, "descriptor": descriptor}

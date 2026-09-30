"""从已登记 Git 对象创建来源验收 Node 使用的不可变工具快照。"""

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
ENTRY = "tools/js/restore_source_existing.mjs"
CONTRACT = "openapi/openapi.json"
MANIFEST_FIELDS = {
    "format_version", "kind", "coordinator_source", "execution_sha", "entry", "contract",
    "runtime_modules", "files",
}
STATIC_IMPORT = re.compile(
    rb"^[ \t]*import(?:[^\"'\r\n]*?[ \t]+from[ \t]*)?[\"']([^\"']+)[\"'][ \t]*;?[ \t]*$",
    re.MULTILINE,
)
DYNAMIC_IMPORT = re.compile(rb"\bimport\s*\(")


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


def _runtime_modules(scripts: dict[str, bytes]) -> list[str]:
    """求唯一入口的静态 ESM 闭包；授权后的运行路径禁止再动态加载磁盘代码。"""
    pending = [ENTRY]
    result = set()
    while pending:
        relative = pending.pop()
        if relative in result:
            continue
        content = scripts.get(relative)
        if content is None:
            raise ValueError("来源验收 ESM 闭包缺少已登记模块")
        if DYNAMIC_IMPORT.search(content):
            raise ValueError("来源验收 ESM 闭包不能在授权后动态加载代码")
        result.add(relative)
        parent = PurePosixPath(relative).parent
        imports = list(STATIC_IMPORT.finditer(content))
        declarations = [line for line in content.splitlines() if re.match(rb"^\s*import\b", line)]
        if len(imports) != len(declarations):
            raise ValueError("来源验收 ESM 闭包包含无法核验的 import 声明")
        for match in imports:
            specifier = match.group(1).decode("utf-8")
            if specifier.startswith("node:"):
                continue
            if not specifier.startswith("./") and not specifier.startswith("../"):
                raise ValueError("来源验收 ESM 闭包不能加载未登记外部包")
            target = parent.joinpath(PurePosixPath(specifier))
            normalized = PurePosixPath(*[part for part in target.parts if part != "."])
            if ".." in normalized.parts or normalized.suffix not in {".js", ".mjs"}:
                raise ValueError("来源验收 ESM 闭包包含越界或隐式模块路径")
            pending.append(normalized.as_posix())
    return sorted(result)


@lru_cache(maxsize=16)
def _committed_sources(
    coordinator_name: str,
    execution_name: str,
    coordinator_sha: str,
    execution_sha: str,
):
    coordinator = Path(coordinator_name)
    execution = Path(execution_name)
    script_paths = [
        path for path in _tree_files(coordinator, coordinator_sha, "tools/js")
        if PurePosixPath(path).suffix in {".js", ".mjs"}
    ]
    scripts = {path: _blob(coordinator, coordinator_sha, path) for path in script_paths}
    if ENTRY not in scripts:
        raise ValueError("来源验收 Git 快照缺少唯一 Node 入口")
    modules = _runtime_modules(scripts)
    contract = _tree_files(execution, execution_sha, CONTRACT)
    if contract != [CONTRACT]:
        raise ValueError("来源验收执行快照缺少唯一 OpenAPI 契约")
    rows = [("coordinator", path, scripts[path]) for path in modules]
    rows.append(("execution", CONTRACT, _blob(execution, execution_sha, CONTRACT)))
    return tuple(modules), tuple(sorted(rows, key=lambda row: row[1]))


def _sources(coordinator: Path, execution: Path, coordinator_sha: str, execution_sha: str):
    modules, rows = _committed_sources(
        str(coordinator.resolve(strict=True)), str(execution.resolve(strict=True)),
        coordinator_sha, execution_sha,
    )
    return list(modules), list(rows)


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
) -> dict:
    """只从两个已登记 commit 的 Git blob 创建固定工具与契约快照。"""
    verify_execution_source(coordinator_source, "来源验收协调器")
    coordinator_sha = coordinator_source["snapshot"]["head"]
    if (
        coordinator_source["snapshot"]["clean"] is not True
        or not isinstance(execution_sha, str)
        or re.fullmatch(r"[a-f0-9]{40}", execution_sha) is None
    ):
        raise ValueError("来源工具 staging 要求干净协调器与精确执行 SHA")
    root = _root(coordinator, directory, existing=False)
    modules, rows = _sources(
        coordinator, execution.resolve(strict=True), coordinator_sha, execution_sha
    )
    _create_directories(root, rows)
    files = []
    for origin, relative, content in rows:
        descriptor = _write_new(root / PurePosixPath(relative), content)
        files.append({"path": relative, "origin": origin, "bytes": descriptor["bytes"],
                      "sha256": descriptor["sha256"]})
    manifest = {
        "format_version": 1,
        "kind": "restore-source-tool-staging",
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "entry": ENTRY,
        "contract": CONTRACT,
        "runtime_modules": modules,
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
    modules, expected_rows = _sources(
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
        expected_files.append({"path": relative, "origin": origin, "bytes": len(content),
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
    expected = {
        "format_version": 1,
        "kind": "restore-source-tool-staging",
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "entry": ENTRY,
        "contract": CONTRACT,
        "runtime_modules": modules,
        "files": expected_files,
    }
    if (
        type(manifest["format_version"]) is not int
        or manifest != expected
        or artifact_snapshot(manifest_path).descriptor() != descriptor
    ):
        raise ValueError("来源工具 staging 清单、来源或摘要不同")
    return {"root": root, "entry": root / ENTRY, "contract": root / CONTRACT,
            "manifest": manifest, "descriptor": descriptor}

"""为正式恢复监控创建并核验不可变的执行输入 staging。"""

from __future__ import annotations

import ast
from contextlib import contextmanager
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys

from artifact_digests import protect_binaries, protect_files
from process_environment import configured
from restore_monitoring_permissions import TOKEN_NAME, read_private_token
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    artifact_snapshot,
    exact_fields,
    read_json_document,
    reject_link_or_reparse,
)


DIRECTORY = "input-staging"
MANIFEST = "manifest.json"
RUNNER = "tools/python/restore_monitoring_delivery.py"
ENVIRONMENT_CHECK = "tools/python/check_python_environment.py"
REQUIREMENTS = "scripts/requirements-ci.txt"
RESOURCES = (
    "deploy/prometheus/ryframe-alerts.yml",
    "deploy/prometheus/ryframe-alerts.test.yml",
)
TOOL_VERSIONS = {
    "prometheus": "prometheus, version 3.5.0",
    "promtool": "promtool, version 3.5.0",
    "alertmanager": "alertmanager, version 0.34.0",
    "amtool": "amtool, version 0.34.0",
}
MANIFEST_FIELDS = {
    "format_version",
    "kind",
    "coordinator",
    "modules",
    "files",
    "python",
    "runner",
    "tools",
    "credential",
}
FILE_FIELDS = {"path", "bytes", "sha256", "device", "inode"}


def _git(root: Path, *arguments: str) -> bytes:
    environment = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    environment.update({"GIT_OPTIONAL_LOCKS": "0", "GIT_NO_REPLACE_OBJECTS": "1"})
    return subprocess.check_output(["git", "-C", str(root), *arguments], env=environment)


def _tree_python(root: Path, revision: str) -> dict[str, bytes]:
    raw = _git(root, "ls-tree", "-r", "-z", revision, "--", "scripts")
    paths = []
    for item in raw.split(b"\0"):
        if not item:
            continue
        try:
            header, encoded = item.split(b"\t", 1)
            mode, kind, _object = header.decode("ascii").split(" ")
            relative = encoded.decode("utf-8")
        except (ValueError, UnicodeDecodeError) as error:
            raise ValueError("监控 staging Git 树包含无效路径") from error
        pure = PurePosixPath(relative)
        if (
            kind != "blob"
            or mode not in {"100644", "100755"}
            or pure.is_absolute()
            or ".." in pure.parts
            or pure.as_posix() != relative
        ):
            raise ValueError("监控 staging Git 树包含非普通文件")
        if pure.suffix == ".py" or relative == REQUIREMENTS:
            paths.append(relative)
    return {path: _git(root, "cat-file", "blob", f"{revision}:{path}") for path in paths}


def _local_imports(relative: str, content: bytes, available: set[str]) -> set[str]:
    try:
        tree = ast.parse(content, filename=relative)
    except (SyntaxError, UnicodeDecodeError) as error:
        raise ValueError(f"监控 staging 无法解析已登记 Python 模块：{relative}") from error
    result = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names = [alias.name for alias in node.names]
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                raise ValueError("监控 staging 不接受隐式相对 Python import")
            names = [node.module] if node.module else []
        elif isinstance(node, ast.Call) and (
            (isinstance(node.func, ast.Name) and node.func.id == "__import__")
            or (isinstance(node.func, ast.Attribute) and node.func.attr == "import_module")
        ):
            raise ValueError("监控 staging 不接受运行期动态 Python import")
        else:
            continue
        for name in names:
            candidate = "tools/python/" + name.split(".", 1)[0] + ".py"
            if candidate in available:
                result.add(candidate)
    return result


@lru_cache(maxsize=8)
def _committed_files(root_name: str, revision: str) -> tuple[tuple[str, bytes], ...]:
    root = Path(root_name)
    sources = _tree_python(root, revision)
    if RUNNER not in sources or ENVIRONMENT_CHECK not in sources or REQUIREMENTS not in sources:
        raise ValueError("监控 staging 的固定入口或 Python 依赖清单缺失")
    pending = [RUNNER, ENVIRONMENT_CHECK]
    modules = set()
    while pending:
        relative = pending.pop()
        if relative in modules:
            continue
        content = sources.get(relative)
        if content is None:
            raise ValueError("监控 staging Python 闭包缺少本地模块")
        modules.add(relative)
        pending.extend(_local_imports(relative, content, set(sources)) - modules)
    selected = modules | {REQUIREMENTS}
    rows = [(path, sources[path]) for path in sorted(selected)]
    for path in RESOURCES:
        rows.append((path, _git(root, "cat-file", "blob", f"{revision}:{path}")))
    return tuple(sorted(rows))


def _relative(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label}路径无效")
    pure = PurePosixPath(value)
    if pure.is_absolute() or ".." in pure.parts or pure.as_posix() != value or not pure.parts:
        raise ValueError(f"{label}路径必须是规范 staging 相对路径")
    return value


def _identity(path: Path, relative: str) -> dict:
    snapshot = artifact_snapshot(path)
    return {
        "path": relative,
        "bytes": snapshot.bytes,
        "sha256": snapshot.sha256,
        "device": snapshot.state[0],
        "inode": snapshot.state[1],
    }


def _write_new(path: Path, content: bytes, relative: str) -> dict:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return _identity(path, relative)


def _create_directories(root: Path, paths: list[str]) -> None:
    root.mkdir()
    parents = {PurePosixPath(path).parent for path in paths}
    expanded = {parent for value in parents for parent in value.parents} | parents
    for relative in sorted(
        (value for value in expanded if value != PurePosixPath(".")),
        key=lambda value: (len(value.parts), value.as_posix()),
    ):
        (root / Path(*relative.parts)).mkdir()


def _ordinary_tool(backend: Path, path: Path, name: str) -> Path:
    if not path.is_absolute():
        raise ValueError(f"监控工具 {name} 必须是绝对路径")
    reject_link_or_reparse(path)
    resolved = path.resolve(strict=True)
    local = (backend / ".local-tests").resolve(strict=True)
    if (
        not resolved.is_file()
        or not resolved.is_relative_to(local)
        or (os.name != "nt" and not os.access(resolved, os.X_OK))
    ):
        raise ValueError(f"监控工具 {name} 必须位于协调器忽略目录")
    return resolved


def _source_python() -> Path:
    configured_python = os.environ.get("RYFRAME_PYTHON", "")
    if not configured_python or not Path(configured_python).is_absolute():
        raise ValueError("监控验收要求显式非空绝对 RYFRAME_PYTHON")
    requested = Path(configured_python).resolve(strict=True)
    if requested != Path(sys.executable).resolve(strict=True):
        raise ValueError("监控验收必须由 RYFRAME_PYTHON 指定的解释器执行")
    reject_link_or_reparse(requested)
    return requested


def _copy_binary(source: Path, target: Path, relative: str) -> dict:
    with source.open("rb") as reader, target.open("xb") as writer:
        while block := reader.read(1024 * 1024):
            writer.write(block)
        writer.flush()
        os.fsync(writer.fileno())
    if os.name != "nt":
        target.chmod(source.stat().st_mode & 0o700)
    return _identity(target, relative)


def _run_command(command: list[str], cwd: Path, run=subprocess.run) -> str:
    completed = run(
        command,
        cwd=cwd,
        env=configured({"NO_PROXY": "127.0.0.1,localhost,::1"}),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=60,
        check=False,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    raw = completed.stdout if isinstance(completed.stdout, bytes) else str(completed.stdout).encode()
    if completed.returncode != 0 or not raw or len(raw) > 64 * 1024:
        raise ValueError("监控 staging 执行输入无法报告固定版本")
    return raw.decode("utf-8", errors="strict").splitlines()[0].strip()


def _version_receipts(root: Path, python: dict, tools: dict, run=subprocess.run) -> tuple[dict, dict]:
    python_path = root / Path(*PurePosixPath(python["path"]).parts)
    check_path = root / Path(*PurePosixPath(ENVIRONMENT_CHECK).parts)
    check = _run_command(
        [str(python_path), "-X", "utf8", "-E", "-s", "-B", str(check_path)], root, run
    )
    version = _run_command([str(python_path), "-E", "-s", "--version"], root, run)
    expected_python = "Python " + sys.version.split()[0]
    if version != expected_python or not check.startswith("Python environment check passed"):
        raise ValueError("监控 staging Python 版本或环境检查不符合固定合同")
    python_receipt = {
        **python,
        "version": sys.version.split()[0],
        "environment_check_sha256": hashlib.sha256(check.encode()).hexdigest(),
    }
    tool_receipts = {}
    for name, binding in tools.items():
        path = root / Path(*PurePosixPath(binding["path"]).parts)
        version = _run_command([str(path), "--version"], root, run)
        if len(version) > 512 or not version.isprintable() or not version.startswith(TOOL_VERSIONS[name]):
            raise ValueError(f"监控工具 {name} 版本不是固定合同")
        tool_receipts[name] = {**binding, "version": version}
    return python_receipt, tool_receipts


def create_staging(
    backend: Path,
    run_directory: Path,
    coordinator: dict,
    credential_path: Path,
    tool_paths: dict[str, Path],
    *,
    run=subprocess.run,
    acl_reader=None,
) -> dict:
    """从干净 Git blob 和受保护二进制建立 create-only staging，清单最后发布。"""
    if set(coordinator) != {"root", "head", "inventory_sha256"} or coordinator["root"] != str(backend):
        raise ValueError("监控 staging 协调来源无效")
    if not isinstance(coordinator["head"], str) or HEX_40.fullmatch(coordinator["head"]) is None:
        raise ValueError("监控 staging 缺少精确协调提交")
    if credential_path != run_directory / TOKEN_NAME:
        raise ValueError("监控凭据必须是本次 run 的唯一固定 token 文件")
    if set(tool_paths) != set(TOOL_VERSIONS):
        raise ValueError("监控 staging 工具集不完整")
    secret, credential = read_private_token(credential_path, acl_reader=acl_reader)
    del secret
    python_source = _source_python()
    sources = {name: _ordinary_tool(backend, path, name) for name, path in tool_paths.items()}
    snapshots = {"python": artifact_snapshot(python_source)}
    snapshots.update({name: artifact_snapshot(path) for name, path in sources.items()})
    bindings = [
        {"path": str(snapshot.path), "sha256": snapshot.sha256} for snapshot in snapshots.values()
    ]
    rows = list(_committed_files(str(backend.resolve(strict=True)), coordinator["head"]))
    python_relative = "python/" + ("python.exe" if os.name == "nt" else "python")
    tool_relatives = {
        name: "tools/" + name + (".exe" if os.name == "nt" else "") for name in TOOL_VERSIONS
    }
    root = run_directory / DIRECTORY
    paths = [relative for relative, _content in rows] + [python_relative, *tool_relatives.values()]
    if root.exists():
        raise ValueError("监控 staging 必须是 create-only 目录")
    with protect_binaries(bindings):
        _create_directories(root, paths)
        files = [_write_new(root / Path(*PurePosixPath(path).parts), content, path) for path, content in rows]
        python = _copy_binary(python_source, root / Path(*PurePosixPath(python_relative).parts), python_relative)
        tools = {
            name: _copy_binary(sources[name], root / Path(*PurePosixPath(relative).parts), relative)
            for name, relative in tool_relatives.items()
        }
        files.extend([python, *tools.values()])
        for snapshot in snapshots.values():
            snapshot.assert_unchanged()
    staged_bindings = [
        {
            "path": str(root / Path(*PurePosixPath(item["path"]).parts)),
            "sha256": item["sha256"],
        }
        for item in files
    ]
    with protect_files(staged_bindings):
        python, tools = _version_receipts(root, python, tools, run)
        for item in files:
            path = root / Path(*PurePosixPath(item["path"]).parts)
            if _identity(path, item["path"]) != item:
                raise ValueError("监控 staging 在首次执行期间发生变化")
    runner = next(item for item in files if item["path"] == RUNNER)
    manifest = {
        "format_version": 1,
        "kind": "restore-monitoring-input-staging",
        "coordinator": coordinator,
        "modules": [path for path, _content in rows if path.endswith(".py")],
        "files": sorted(files, key=lambda item: item["path"]),
        "python": python,
        "runner": runner,
        "tools": tools,
        "credential": {**credential, "path": TOKEN_NAME},
    }
    raw = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    _write_new(root / MANIFEST, raw, MANIFEST)
    return artifact_snapshot(root / MANIFEST).descriptor()


def _file_row(value: object, label: str) -> dict:
    value = exact_fields(value, FILE_FIELDS, label)
    _relative(value["path"], label)
    if (
        type(value["bytes"]) is not int
        or value["bytes"] <= 0
        or not isinstance(value["sha256"], str)
        or HEX_64.fullmatch(value["sha256"]) is None
        or type(value["device"]) is not int
        or type(value["inode"]) is not int
    ):
        raise ValueError(f"{label}描述无效")
    return value


def _verify_members(root: Path, files: list[dict]) -> None:
    expected_files = {MANIFEST, *(item["path"] for item in files)}
    expected_directories = {"."}
    for relative in expected_files:
        expected_directories.update(
            parent.as_posix()
            for parent in PurePosixPath(relative).parents
            if parent != PurePosixPath(".")
        )
    observed_files, observed_directories = set(), {"."}
    reject_link_or_reparse(root)
    for path in root.rglob("*"):
        reject_link_or_reparse(path)
        relative = path.relative_to(root).as_posix()
        if path.is_file():
            observed_files.add(relative)
        elif path.is_dir():
            observed_directories.add(relative)
        else:
            raise ValueError("监控 staging 包含非普通文件")
    if observed_files != expected_files or observed_directories != expected_directories:
        raise ValueError("监控 staging 包含缺失或未知成员")


def _execution_fields(manifest: dict, files: list[dict]) -> tuple[dict, dict, dict]:
    python = exact_fields(
        manifest["python"],
        FILE_FIELDS | {"version", "environment_check_sha256"},
        "监控 staging Python",
    )
    _file_row({key: python[key] for key in FILE_FIELDS}, "监控 staging Python")
    if (
        {key: python[key] for key in FILE_FIELDS} not in files
        or not isinstance(python["version"], str)
        or not python["version"]
        or not isinstance(python["environment_check_sha256"], str)
        or HEX_64.fullmatch(python["environment_check_sha256"]) is None
    ):
        raise ValueError("监控 staging Python 版本绑定无效")
    runner = _file_row(manifest["runner"], "监控 staging runner")
    if runner["path"] != RUNNER or runner not in files:
        raise ValueError("监控 staging runner 不唯一")
    tools = exact_fields(manifest["tools"], set(TOOL_VERSIONS), "监控 staging 工具")
    for name, tool in tools.items():
        exact_fields(tool, FILE_FIELDS | {"version"}, f"监控 staging 工具 {name}")
        if (
            _file_row({key: tool[key] for key in FILE_FIELDS}, f"监控 staging 工具 {name}")
            not in files
            or not isinstance(tool["version"], str)
            or not tool["version"].startswith(TOOL_VERSIONS[name])
        ):
            raise ValueError(f"监控 staging 工具 {name} 绑定无效")
    return python, runner, tools


def _inspect(
    run_directory: Path,
    descriptor: dict,
    coordinator: dict,
    *,
    acl_reader=None,
) -> dict:
    root = run_directory / DIRECTORY
    manifest_path = root / MANIFEST
    try:
        document = read_json_document(manifest_path)
    except (OSError, UnicodeError, ValueError) as error:
        raise ValueError("监控 staging 清单不是有效 UTF-8 JSON") from error
    actual_manifest = {
        "path": str(document.path),
        "bytes": len(document.raw),
        "sha256": document.sha256,
    }
    if actual_manifest != descriptor:
        raise ValueError("监控 staging 清单与绑定摘要不同")
    manifest = document.value
    exact_fields(manifest, MANIFEST_FIELDS, "监控 staging 清单")
    if (
        manifest["format_version"] != 1
        or manifest["kind"] != "restore-monitoring-input-staging"
        or manifest["coordinator"] != coordinator
        or not isinstance(manifest["modules"], list)
        or manifest["modules"] != sorted(set(manifest["modules"]))
    ):
        raise ValueError("监控 staging 清单版本或协调来源无效")
    files = [_file_row(value, "监控 staging 文件") for value in manifest["files"]]
    if files != sorted(files, key=lambda item: item["path"]) or len(files) != len({item["path"] for item in files}):
        raise ValueError("监控 staging 文件清单必须有序且唯一")
    expected_modules = sorted(item["path"] for item in files if item["path"].endswith(".py"))
    paths = {item["path"] for item in files}
    if (
        manifest["modules"] != expected_modules
        or REQUIREMENTS not in paths
        or not set(RESOURCES).issubset(paths)
    ):
        raise ValueError("监控 staging Python 闭包或依赖清单不完整")
    _verify_members(root, files)
    for row in files:
        path = root / Path(*PurePosixPath(row["path"]).parts)
        if _identity(path, row["path"]) != row:
            raise ValueError("监控 staging 执行字节或文件身份与清单不同")
    python, runner, tools = _execution_fields(manifest, files)
    credential = exact_fields(
        manifest["credential"],
        {"path", "bytes", "sha256", "device", "inode", "security_sha256"},
        "监控 staging 凭据",
    )
    if credential["path"] != TOKEN_NAME:
        raise ValueError("监控 staging 凭据路径无效")
    secret, actual_credential = read_private_token(run_directory / TOKEN_NAME, acl_reader=acl_reader)
    if {**actual_credential, "path": TOKEN_NAME} != credential:
        raise ValueError("监控 staging 凭据字节、身份或权限不同")
    file_bindings = [
        {"path": str(root / Path(*PurePosixPath(item["path"]).parts)), "sha256": item["sha256"]}
        for item in files
    ]
    file_bindings.extend(
        [
            {"path": str(manifest_path), "sha256": descriptor["sha256"]},
            {"path": str(run_directory / TOKEN_NAME), "sha256": credential["sha256"]},
        ]
    )
    return {
        "root": root,
        "manifest": manifest,
        "python": root / Path(*PurePosixPath(python["path"]).parts),
        "runner": root / Path(*PurePosixPath(runner["path"]).parts),
        "tools": {
            name: root / Path(*PurePosixPath(tool["path"]).parts) for name, tool in tools.items()
        },
        "credential": run_directory / TOKEN_NAME,
        "secret": secret,
        "bindings": file_bindings,
    }


@contextmanager
def verified_staging_execution(
    run_directory: Path,
    descriptor: dict,
    coordinator: dict,
    *,
    acl_reader=None,
    private_environment: dict[str, str] | None = None,
):
    """锁定全部执行字节后再次核验；调用方只能消费返回的 staging 路径。"""
    before = _inspect(run_directory, descriptor, coordinator, acl_reader=acl_reader)
    with protect_files(before["bindings"]):
        current = _inspect(run_directory, descriptor, coordinator, acl_reader=acl_reader)
        comparable = {key: current[key] for key in ("root", "manifest", "python", "runner", "tools", "credential")}
        expected = {key: before[key] for key in comparable}
        if comparable != expected or current["secret"] != before["secret"]:
            raise ValueError("监控 staging 在进入执行阶段时发生变化")
        exposed = {
            key: current[key]
            for key in ("root", "manifest", "python", "runner", "tools", "credential")
        }
        exposed["runner_command"] = [
            str(current["python"]),
            "-X",
            "utf8",
            "-E",
            "-s",
            "-B",
            str(current["runner"]),
        ]
        if private_environment is not None:
            if (
                not isinstance(private_environment, dict)
                or any(
                    not isinstance(key, str) or not isinstance(value, str)
                    for key, value in private_environment.items()
                )
                or private_environment.get("APP_MONITOR_METRICS_BEARER_TOKEN") != current["secret"]
            ):
                raise ValueError("监控 staging 私有环境与固定凭据不同")
            exposed["environment"] = configured(private_environment)
        primary = None
        try:
            yield exposed
        except BaseException as error:
            primary = error
            raise
        finally:
            try:
                after = _inspect(run_directory, descriptor, coordinator, acl_reader=acl_reader)
                final = {key: after[key] for key in comparable}
                if final != expected or after["secret"] != before["secret"]:
                    raise ValueError("监控 staging 在执行阶段发生变化")
            except BaseException as cleanup:
                if primary is None:
                    raise
                primary.add_note(f"监控 staging 执行失败后的复核同时失败：{type(cleanup).__name__}")


def verify_staging(
    run_directory: Path,
    descriptor: dict,
    coordinator: dict,
    *,
    run=subprocess.run,
    acl_reader=None,
) -> dict:
    with verified_staging_execution(
        run_directory, descriptor, coordinator, acl_reader=acl_reader
    ) as execution:
        python, tools = _version_receipts(
            execution["root"], execution["manifest"]["python"], execution["manifest"]["tools"], run
        )
        if python != execution["manifest"]["python"] or tools != execution["manifest"]["tools"]:
            raise ValueError("监控 staging 实际执行版本与清单不同")
        root = execution["root"]

        def receipt(value: dict) -> dict:
            relative = Path(*PurePosixPath(value["path"]).parts)
            return {**value, "path": str(root / relative)}

        return {
            "python": receipt(python),
            "runner": receipt(execution["manifest"]["runner"]),
            "tools": {name: receipt(value) for name, value in tools.items()},
            "credential": {**execution["manifest"]["credential"], "path": str(execution["credential"])},
        }

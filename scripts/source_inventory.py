"""只读采集 Git 快照、完整文件清单和来源分类，不依赖复制或恢复流程。"""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import stat
import subprocess

from artifact_digests import file_digest

FRONTEND_ENVIRONMENT_PATHS = (".env", ".env.local", ".env.production", ".env.production.local")


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


def _frontend_product(relative: str) -> bool:
    path = PurePosixPath(relative)
    if path.parts[0] in {"src", "public", "openapi"}:
        return True
    if relative in {
        ".env", ".node-version", "index.html", "package.json", "pnpm-lock.yaml",
        "pnpm-workspace.yaml", "vite.config.ts", "tsconfig.json", "tsconfig.app.json",
        "tsconfig.base.json", "tsconfig.test.json",
    }:
        return True
    if (path.parts[0] in {"scripts", "tests", ".github"}
            or relative in {"eslint.config.js", "playwright.config.ts", "playwright.real.config.ts",
                            "prettier.config.mjs", "stylelint.config.js", "vitest.config.ts",
                            ".editorconfig", ".gitattributes", ".gitignore", ".prettierignore",
                            ".env.production.example", "ARCHITECTURE.md", "CHANGELOG.md", "LICENSE",
                            "README.md"}):
        return False
    # 新增且尚未分类的文件按产品输入处理，不能因工具重构漏掉实际构建依赖。
    return True


def _backend_product_roles(relative: str) -> tuple[str, ...]:
    path = PurePosixPath(relative)
    both = ("api", "worker")
    if path.parts[0] in {"scripts", "xtask", "tests", ".github", "architecture", "docs", "deploy"}:
        return ()
    if len(path.parts) >= 3 and path.parts[0] == "crates" and path.parts[2] == "tests":
        return ()
    if path.parts[:2] == ("crates", "ryframe-api"):
        return ("api",)
    if path.parts[:2] == ("crates", "ryframe-generator"):
        return ()
    if path.parts[:3] == ("crates", "ryframe", "src"):
        nested = path.parts[3:]
        if not nested:
            return both
        if nested[0] == "main.rs":
            return ("api",)
        if nested[0] == "reset":
            return ()
        if nested[0] == "bin":
            if len(nested) >= 2 and (nested[1] == "ryframe_worker.rs" or nested[1] == "ryframe_worker"):
                return ("worker",)
            return ()
        return both
    if path.parts[:2] == ("crates", "ryframe"):
        return both
    if path.parts[0] == "crates":
        return both
    if path.parts[0] in {".cargo", "vendor", "catalog", "config", "locales", "sql"}:
        return both
    if path.parts[0] == "openapi":
        return ("api",)
    if relative in {"Cargo.lock", "Cargo.toml", "rust-toolchain.toml"}:
        return both
    if relative in {"CHANGELOG.md", "LICENSE", "README.md", ".dockerignore", ".editorconfig",
                    ".gitattributes", ".gitignore", "deny.toml", "rustfmt.toml"}:
        return ()
    # 未知路径保守绑定两个产品角色，避免错误复用旧二进制。
    return both


def _tool_source(relative: str, repository: str) -> bool:
    path = PurePosixPath(relative)
    if repository == "backend":
        return (path.parts[0] in {"scripts", "xtask", "tests", ".github", "architecture"}
                or len(path.parts) >= 3 and path.parts[0] == "crates" and path.parts[2] == "tests"
                or relative in {"deny.toml", "rustfmt.toml"})
    return (path.parts[0] in {"scripts", "tests", ".github"}
            or relative in {"eslint.config.js", "playwright.config.ts", "playwright.real.config.ts",
                            "prettier.config.mjs", "stylelint.config.js", "vitest.config.ts"})


def _source_group(files: list[dict], paths: list[str]) -> dict:
    selected = {item["path"]: item for item in files}
    values = [selected[path] for path in paths]
    return {"sha256": canonical_digest(values), "files": paths}


def validate_inventory(inventory: object) -> dict:
    if not isinstance(inventory, dict) or set(inventory) != {"source", "files", "guard"}:
        raise ValueError("完整来源清单字段无效")
    source, guard = inventory["source"], inventory["guard"]
    if not isinstance(source, dict) or set(source) != {"snapshot", "worktree_fingerprint"}:
        raise ValueError("完整来源快照字段无效")
    snapshot = source["snapshot"]
    if not isinstance(snapshot, dict) or set(snapshot) != {"head", "patch_sha256", "files", "clean"}:
        raise ValueError("完整来源 Git 快照字段无效")
    if (not isinstance(snapshot["head"], str) or not re.fullmatch(r"[a-f0-9]{40}", snapshot["head"])
            or not isinstance(snapshot["patch_sha256"], str)
            or not re.fullmatch(r"[a-f0-9]{64}", snapshot["patch_sha256"])
            or type(snapshot["clean"]) is not bool
            or not isinstance(source["worktree_fingerprint"], str)
            or not re.fullmatch(r"sha256:[a-f0-9]{64}", source["worktree_fingerprint"])):
        raise ValueError("完整来源 Git 快照内容无效")
    if (not isinstance(guard, dict) or set(guard) != {"head", "index_sha256", "modes_sha256"}
            or guard["head"] != snapshot["head"]
            or any(not isinstance(guard[field], str) or not re.fullmatch(r"[a-f0-9]{64}", guard[field])
                   for field in ("index_sha256", "modes_sha256"))):
        raise ValueError("完整来源 Git guard 无效")
    _validated_source_files(snapshot["files"], "未跟踪来源文件")
    _validated_source_files(inventory["files"], "完整来源文件")
    return inventory


def _validated_source_files(value: object, label: str) -> list[dict]:
    if not isinstance(value, list):
        raise ValueError(f"{label}清单无效")
    previous = ""
    for item in value:
        if (not isinstance(item, dict) or set(item) != {"path", "sha256"}
                or not isinstance(item["path"], str) or item["path"] <= previous
                or not isinstance(item["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", item["sha256"])):
            raise ValueError(f"{label}必须是无重复的排序文件摘要")
        source_domain(item["path"])
        previous = item["path"]
    return value


def build_source_domains(inventory: dict, repository: str) -> dict:
    """从同一份完整清单派生按产物角色的产品输入和验收工具输入。"""
    validate_inventory(inventory)
    if repository not in {"backend", "frontend"}:
        raise ValueError("构建来源仓库类型无效")
    files = inventory["files"]
    roles = {role: [] for role in (("api", "worker") if repository == "backend" else ("frontend",))}
    tool_paths = []
    for item in files:
        relative = item["path"]
        selected_roles = _backend_product_roles(relative) if repository == "backend" else (
            ("frontend",) if _frontend_product(relative) else ()
        )
        for role in selected_roles:
            roles[role].append(relative)
        if _tool_source(relative, repository):
            tool_paths.append(relative)
    return {
        "product": {role: _source_group(files, paths) for role, paths in roles.items()},
        "tools": _source_group(files, tool_paths),
        "full": copy.deepcopy(inventory),
    }


def validate_build_source_domains(value: object, repository: str) -> dict:
    expected_roles = {"api", "worker"} if repository == "backend" else {"frontend"}
    if (not isinstance(value, dict) or set(value) != {"product", "tools", "full"}
            or not isinstance(value["product"], dict) or set(value["product"]) != expected_roles):
        raise ValueError("构建来源分域字段无效")
    expected = build_source_domains(validate_inventory(value["full"]), repository)
    if value != expected:
        raise ValueError("构建来源分域与完整来源清单不一致")
    return value


def frontend_environment_files(root: Path) -> list[dict]:
    """绑定 Vite production 模式会读取的四个环境文件；只记录文件摘要。"""
    result = []
    for relative in FRONTEND_ENVIRONMENT_PATHS:
        path = root / relative
        if not path.exists():
            continue
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError("Vite 环境文件必须是前端仓库内的普通文件")
        result.append({"path": relative, "sha256": file_digest(path)["sha256"]})
    return result


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

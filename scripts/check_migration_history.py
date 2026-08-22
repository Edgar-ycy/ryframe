#!/usr/bin/env python3
"""校验迁移只追加，并管理提交后不可变的迁移冻结清单。"""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


MIGRATION_NAME = re.compile(r"^m(?P<stamp>\d{8}_\d{6})_(?P<label>[a-z][a-z0-9_]*)$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_GIT_REF = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")
CONTROL_BASELINE = "m20260820_000000_control_baseline"
TENANT_BASELINE = "m20260820_000000_tenant_baseline"
CONTROL_PREFIX = "crates/ryframe-db/src/migration/"
TENANT_PREFIX = "crates/ryframe-tenant-db/src/migration/"
CONTROL_GENERATED_PREFIX = "crates/ryframe-db/src/generated/"
TENANT_GENERATED_PREFIX = "crates/ryframe-tenant-db/src/generated/"
LOCK_RELATIVE_PATH = "catalog/migrations.lock.toml"
ROLL_FORWARD_DOWN = re.compile(
    r'^(?:return)?Err\(DbErr::Custom\("(?P<message>(?:\\.|[^"\\])*)"\.into\(\),?\)\);?$'
)


@dataclass(frozen=True)
class Migration:
    name: str
    source: Path
    module_registry: Path
    migrator_registry: Path

    def source_files(self) -> list[Path]:
        if self.source.name == "mod.rs" and self.source.parent.name == self.name:
            return sorted(path for path in self.source.parent.rglob("*.rs") if path.is_file())
        return [self.source]


@dataclass(frozen=True)
class LockedFile:
    relative: str
    path: Path
    expected_sha256: str
    storage: str
    target: str


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(128 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def safe_locked_path(root: Path, value: object) -> Path:
    if not isinstance(value, str):
        raise ValueError("冻结文件 path 必须是字符串")
    relative = PurePosixPath(value)
    if relative.is_absolute() or ".." in relative.parts or "." in relative.parts:
        raise ValueError(f"冻结文件路径不安全：{value!r}")
    if not any(
        value.startswith(prefix)
        for prefix in (
            CONTROL_PREFIX,
            TENANT_PREFIX,
            CONTROL_GENERATED_PREFIX,
            TENANT_GENERATED_PREFIX,
        )
    ):
        raise ValueError(f"冻结文件不属于迁移目录：{value}")
    if relative.suffix != ".rs":
        raise ValueError(f"冻结文件必须是 Rust 迁移源码：{value}")
    lock_identity(value)
    return root.joinpath(*relative.parts)


def lock_identity(relative: str) -> tuple[str, str]:
    """从受控路径推导稳定存储域与迁移目标，禁止 lock 自行声明语义。"""
    path = PurePosixPath(relative)
    value = path.as_posix()
    if value.startswith(CONTROL_PREFIX):
        storage = "control"
        remainder = path.parts[len(PurePosixPath(CONTROL_PREFIX).parts) :]
    elif value.startswith(TENANT_PREFIX):
        storage = "tenant-data"
        remainder = path.parts[len(PurePosixPath(TENANT_PREFIX).parts) :]
    elif value.startswith(CONTROL_GENERATED_PREFIX):
        storage = "control"
        remainder = path.parts[len(PurePosixPath(CONTROL_GENERATED_PREFIX).parts) :]
        if len(remainder) != 2 or remainder[1] != "migration.rs":
            raise ValueError(f"生成迁移路径必须是 <resource>/migration.rs：{relative}")
        return storage, f"resource:{remainder[0]}:initial"
    elif value.startswith(TENANT_GENERATED_PREFIX):
        storage = "tenant-data"
        remainder = path.parts[len(PurePosixPath(TENANT_GENERATED_PREFIX).parts) :]
        if len(remainder) != 2 or remainder[1] != "migration.rs":
            raise ValueError(f"生成迁移路径必须是 <resource>/migration.rs：{relative}")
        return storage, f"resource:{remainder[0]}:initial"
    else:
        raise ValueError(f"冻结文件不属于迁移目录：{relative}")

    if not remainder:
        raise ValueError(f"冻结文件没有迁移目标：{relative}")
    target = remainder[0] if len(remainder) > 1 else PurePosixPath(remainder[0]).stem
    if not MIGRATION_NAME.fullmatch(target):
        raise ValueError(f"冻结文件无法推导合法迁移目标：{relative}")
    return storage, target


def parse_lock(
    root: Path, source: str, content: bytes
) -> tuple[dict[str, object] | None, list[LockedFile], list[str]]:
    try:
        document = tomllib.loads(content.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        return None, [], [f"无法解析迁移冻结清单 {source}: {error}"]
    if document.get("format_version") != 1:
        return document, [], ["catalog/migrations.lock.toml format_version 必须为 1"]

    errors: list[str] = []
    entries: list[LockedFile] = []
    seen: set[str] = set()
    files = document.get("files")
    if not isinstance(files, list) or not files:
        return document, [], ["迁移冻结清单必须包含至少一个 [[files]]"]
    for index, entry in enumerate(files):
        if not isinstance(entry, dict):
            errors.append(f"files[{index}] 必须是对象")
            continue
        value = entry.get("path")
        expected = entry.get("sha256")
        try:
            path = safe_locked_path(root, value)
        except ValueError as error:
            errors.append(str(error))
            continue
        relative = path.relative_to(root).as_posix()
        if relative in seen:
            errors.append(f"冻结文件重复：{relative}")
            continue
        seen.add(relative)
        if not isinstance(expected, str) or not SHA256.fullmatch(expected):
            errors.append(f"{relative} 的 sha256 必须是 64 位小写十六进制")
            continue
        expected_storage, expected_target = lock_identity(relative)
        storage = entry.get("storage")
        target = entry.get("target")
        if storage != expected_storage:
            errors.append(
                f"{relative} 的 storage 必须是从路径推导的 {expected_storage!r}"
            )
            continue
        if target != expected_target:
            errors.append(f"{relative} 的 target 必须是从路径推导的 {expected_target!r}")
            continue
        entries.append(LockedFile(relative, path, expected, storage, target))
    return document, entries, errors


def load_lock(
    root: Path, lock_path: Path
) -> tuple[dict[str, object] | None, list[LockedFile], list[str]]:
    try:
        content = lock_path.read_bytes()
    except OSError as error:
        return None, [], [f"无法读取迁移冻结清单 {lock_path}: {error}"]
    return parse_lock(root, str(lock_path), content)


def verify_lock(root: Path, lock_path: Path) -> list[str]:
    _document, entries, errors = load_lock(root, lock_path)
    for entry in entries:
        if not entry.path.is_file():
            errors.append(f"冻结迁移被删除或改名：{entry.relative}")
            continue
        actual = sha256(entry.path)
        if actual != entry.expected_sha256:
            errors.append(
                f"冻结迁移被修改：{entry.relative}\n"
                "  历史迁移不可编辑；请新建追加迁移并采用 roll-forward 修复"
            )
    return errors


def discover(root: Path) -> list[Migration]:
    control_dir = root / CONTROL_PREFIX
    tenant_dir = root / TENANT_PREFIX
    migrations: list[Migration] = []
    for entry in control_dir.iterdir():
        if entry.is_dir() and MIGRATION_NAME.fullmatch(entry.name):
            migrations.append(
                Migration(
                    entry.name,
                    entry / "mod.rs",
                    control_dir / "mod.rs",
                    control_dir / "mod.rs",
                )
            )
        elif entry.is_file() and entry.suffix == ".rs" and MIGRATION_NAME.fullmatch(entry.stem):
            migrations.append(
                Migration(
                    entry.stem,
                    entry,
                    control_dir / "mod.rs",
                    control_dir / "mod.rs",
                )
            )
    for entry in tenant_dir.iterdir():
        if entry.is_file() and entry.suffix == ".rs" and MIGRATION_NAME.fullmatch(entry.stem):
            migrations.append(
                Migration(
                    entry.stem,
                    entry,
                    tenant_dir / "mod.rs",
                    tenant_dir / "runtime.rs",
                )
            )
    return sorted(migrations, key=lambda migration: migration.name)


def discover_generated(root: Path) -> list[Path]:
    paths: list[Path] = []
    for prefix in (CONTROL_GENERATED_PREFIX, TENANT_GENERATED_PREFIX):
        directory = root / prefix
        if directory.is_dir():
            paths.extend(
                path
                for path in directory.glob("*/migration.rs")
                if path.is_file()
            )
    return sorted(paths)


def verify_generated_registry_wiring(root: Path) -> list[str]:
    """确保生成迁移聚合真正进入两个运行时 Migrator，而不只是生成孤立函数。"""
    errors: list[str] = []
    required = [
        (
            "crates/ryframe-db/src/lib.rs",
            "pub mod generated;",
            "控制库 crate 必须一次性导出 generated 模块",
        ),
        (
            "crates/ryframe-db/src/generated/mod.rs",
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>>",
            "控制库 generated 聚合必须提供 migrations()",
        ),
        (
            "crates/ryframe-db/src/migration/mod.rs",
            "migrations.extend(crate::generated::migrations());",
            "控制库 Migrator 必须消费 generated::migrations()",
        ),
        (
            "crates/ryframe-tenant-db/src/lib.rs",
            "pub mod generated;",
            "租户库 crate 必须一次性导出 generated 模块",
        ),
        (
            "crates/ryframe-tenant-db/src/generated/mod.rs",
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>>",
            "租户库 generated 聚合必须提供 migrations()",
        ),
        (
            "crates/ryframe-tenant-db/src/migration/runtime.rs",
            "migrations.extend(crate::generated::migrations());",
            "租户库运行时 Migrator 必须消费 generated::migrations()",
        ),
    ]
    for relative, marker, message in required:
        path = root / relative
        try:
            source = path.read_text(encoding="utf-8")
        except OSError as error:
            errors.append(f"{message}：无法读取 {relative}：{error}")
            continue
        if marker not in source:
            errors.append(f"{message}：{relative}")
    return errors


def validate_git_ref(value: str) -> str:
    if not SAFE_GIT_REF.fullmatch(value) or ".." in value or "@{" in value:
        raise RuntimeError(f"受信 Git ref 不安全：{value!r}")
    return value


def git_head_paths(root: Path, trusted_ref: str = "HEAD") -> set[str]:
    trusted_ref = validate_git_ref(trusted_ref)
    result = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            trusted_ref,
            "--",
            CONTROL_PREFIX,
            TENANT_PREFIX,
            CONTROL_GENERATED_PREFIX,
            TENANT_GENERATED_PREFIX,
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if result.returncode != 0:
        if (root / ".git").exists():
            raise RuntimeError(
                f"无法从受信 Git ref `{trusted_ref}` 读取迁移，拒绝推断冻结边界"
            )
        return set()
    return {
        value.decode("utf-8").replace("\\", "/")
        for value in result.stdout.split(b"\0")
        if value
    }


def git_head_file(root: Path, relative: str, trusted_ref: str = "HEAD") -> bytes | None:
    """读取受信 ref blob；路径不存在与 Git 无法读取必须区分。"""
    if not (root / ".git").exists():
        return None
    trusted_ref = validate_git_ref(trusted_ref)
    listed = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            trusted_ref,
            "--",
            relative,
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if listed.returncode != 0:
        raise RuntimeError(
            f"无法从受信 Git ref `{trusted_ref}` 读取迁移冻结基准，拒绝信任工作树 lock"
        )
    paths = {
        value.decode("utf-8").replace("\\", "/")
        for value in listed.stdout.split(b"\0")
        if value
    }
    if relative not in paths:
        return None
    blob = subprocess.run(
        ["git", "-C", str(root), "show", f"{trusted_ref}:{relative}"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if blob.returncode != 0:
        raise RuntimeError(f"无法读取受信 Git ref `{trusted_ref}` 中的文件：{relative}")
    return blob.stdout


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def lock_signature(entry: LockedFile) -> tuple[str, str, str, str]:
    return entry.relative, entry.expected_sha256, entry.storage, entry.target


def verify_bootstrap_lock(
    root: Path,
    entries: list[LockedFile],
    head_paths: set[str],
    trusted_ref: str,
) -> list[str]:
    """首次引入 lock 仅允许冻结受信 ref 中既有的两个基线。"""
    if not (root / ".git").exists():
        return []
    errors: list[str] = []
    by_path = {entry.relative: entry for entry in entries}
    for relative in sorted(head_paths):
        try:
            _storage, target = lock_identity(relative)
        except ValueError:
            continue
        if target not in {CONTROL_BASELINE, TENANT_BASELINE}:
            errors.append(
                f"首次引入 migrations.lock.toml 时发现 HEAD 已有未冻结历史迁移：{relative}；"
                "只能从未提交的新追加迁移开始冻结"
            )
            continue
        entry = by_path.get(relative)
        if entry is None:
            errors.append(f"首次引入 migrations.lock.toml 必须完整冻结 HEAD 基线源码：{relative}")
            continue
        try:
            blob = git_head_file(root, relative, trusted_ref)
        except RuntimeError as error:
            errors.append(str(error))
            continue
        if blob is None or sha256_bytes(blob) != entry.expected_sha256:
            errors.append(
                f"首次 lock 中的基线 hash 与受信 HEAD 不一致：{relative}；"
                "不能同时修改历史源码和 lock"
            )
    return errors


def verify_trusted_head_lock(
    root: Path,
    document: dict[str, object],
    entries: list[LockedFile],
    head_paths: set[str],
    trusted_ref: str = "HEAD",
) -> list[str]:
    """当前 lock 只能在受信 ref lock 后追加，旧条目不能被重写。"""
    try:
        head_content = git_head_file(root, LOCK_RELATIVE_PATH, trusted_ref)
    except RuntimeError as error:
        return [str(error)]
    if head_content is None:
        return verify_bootstrap_lock(root, entries, head_paths, trusted_ref)

    head_document, head_entries, errors = parse_lock(
        root, f"HEAD:{LOCK_RELATIVE_PATH}", head_content
    )
    if errors or head_document is None:
        return [*errors, "Git HEAD 中的迁移冻结清单无效，拒绝以工作树内容覆盖"]
    if document.get("frozen_at") != head_document.get("frozen_at"):
        errors.append("migrations.lock.toml 的 frozen_at 已进入 HEAD 后不得修改")
    if len(entries) < len(head_entries):
        errors.append("migrations.lock.toml 不得删除 Git HEAD 中的冻结条目")
        return errors
    for index, head_entry in enumerate(head_entries):
        current = entries[index]
        if lock_signature(current) != lock_signature(head_entry):
            errors.append(
                f"migrations.lock.toml 的 HEAD 条目 #{index + 1} 被修改或重排："
                f"{head_entry.relative}；冻结清单只能在末尾追加"
            )
        try:
            blob = git_head_file(root, head_entry.relative, trusted_ref)
        except RuntimeError as error:
            errors.append(str(error))
            continue
        if blob is None or sha256_bytes(blob) != head_entry.expected_sha256:
            errors.append(
                f"Git HEAD 中的冻结源码与受信 lock 不一致：{head_entry.relative}"
            )
    trusted_targets = {(entry.storage, entry.target) for entry in head_entries}
    for entry in entries[len(head_entries) :]:
        if entry.relative in head_paths:
            errors.append(
                f"拒绝给已存在于 HEAD 的历史迁移补录 lock：{entry.relative}；"
                "只能冻结当前工作树中新建且尚未提交的迁移"
            )
        if (entry.storage, entry.target) in trusted_targets:
            errors.append(
                f"新增 lock 条目复用了受信迁移目标 {entry.storage}/{entry.target}："
                f"{entry.relative}；已冻结迁移不得增加源码文件"
            )
    return errors


def migration_relative_paths(root: Path, migration: Migration) -> list[str]:
    return [path.relative_to(root).as_posix() for path in migration.source_files()]


def rejects_down_with_roll_forward(source: str) -> bool:
    """只接受直接返回错误的 down，避免注释或字符串造成假绿。"""
    function = re.search(
        r"async\s+fn\s+down\b[^{}]*\{(?P<body>[^{}]*)\}", source, re.DOTALL
    )
    if function is None:
        return False
    body = re.sub(r"\s+", "", function.group("body"))
    rejection = ROLL_FORWARD_DOWN.fullmatch(body)
    return rejection is not None and "追加" in rejection.group("message")


def verify_append_only(
    root: Path,
    locked_paths: set[str] | None = None,
    *,
    require_frozen: bool = False,
    head_paths: set[str] | None = None,
) -> list[str]:
    errors: list[str] = []
    migrations = discover(root)
    names = [migration.name for migration in migrations]
    enforce_lock = locked_paths is not None
    locked_paths = locked_paths or set()
    if head_paths is None:
        try:
            head_paths = git_head_paths(root)
        except RuntimeError as error:
            return [str(error)]
    for baseline in [CONTROL_BASELINE, TENANT_BASELINE]:
        if baseline not in names:
            errors.append(f"缺少冻结基线迁移：{baseline}")

    registry_orders: dict[Path, list[tuple[str, int]]] = {}
    for migration in migrations:
        match = MIGRATION_NAME.fullmatch(migration.name)
        assert match is not None
        is_control = migration.source.is_relative_to(root / CONTROL_PREFIX)
        baseline = CONTROL_BASELINE if is_control else TENANT_BASELINE
        if migration.name != baseline and migration.name <= baseline:
            errors.append(f"追加迁移时间必须晚于基线：{migration.name}")
        if not migration.source.is_file():
            errors.append(f"迁移缺少入口文件：{migration.source.relative_to(root).as_posix()}")
            continue

        relative_paths = migration_relative_paths(root, migration)
        any_frozen = any(path in locked_paths for path in relative_paths)
        for relative in relative_paths:
            if enforce_lock and (migration.name == baseline or any_frozen):
                if relative not in locked_paths:
                    errors.append(f"冻结迁移新增了未冻结源码：{relative}")
            elif enforce_lock and relative in head_paths:
                errors.append(
                    f"已提交迁移未进入冻结清单：{relative}；"
                    "禁止用自动检查接受历史文件，请从新建迁移的工作树执行 `cargo migrate freeze`"
                )
            elif enforce_lock and require_frozen:
                errors.append(f"待提交迁移尚未冻结：{relative}；请先运行 `cargo migrate freeze`")

        source = migration.source.read_text(encoding="utf-8")
        if migration.name != baseline:
            if not rejects_down_with_roll_forward(source):
                errors.append(
                    f"{migration.name} 的 down 必须返回说明追加修复策略的 DbErr::Custom，"
                    "且不得执行其他逻辑"
                )

        module_registry = migration.module_registry.read_text(encoding="utf-8")
        if f"mod {migration.name};" not in module_registry:
            errors.append(f"迁移未在模块中注册：{migration.name}")
        migrator_registry = migration.migrator_registry.read_text(encoding="utf-8")
        marker = f"{migration.name}::Migration"
        position = migrator_registry.find(marker)
        if position < 0:
            errors.append(f"迁移未加入 Migrator 顺序：{migration.name}")
        else:
            registry_orders.setdefault(migration.migrator_registry, []).append(
                (migration.name, position)
            )

    for registry, entries in registry_orders.items():
        names_in_file = [name for name, _position in sorted(entries, key=lambda item: item[1])]
        if names_in_file != sorted(names_in_file):
            errors.append(
                f"Migrator 必须按迁移名称递增注册：{registry.relative_to(root).as_posix()}"
            )
    for source in discover_generated(root):
        relative = source.relative_to(root).as_posix()
        if enforce_lock and relative in head_paths and relative not in locked_paths:
            errors.append(
                f"已提交迁移未进入冻结清单：{relative}；"
                "资源初始迁移必须在提交前执行 `cargo migrate freeze`"
            )
        elif enforce_lock and require_frozen and relative not in locked_paths:
            errors.append(f"待提交迁移尚未冻结：{relative}；请先运行 `cargo migrate freeze`")
        source_text = source.read_text(encoding="utf-8")
        if "INITIAL_RESOURCE_MIGRATION" not in source_text:
            errors.append(f"generated migration 缺少不可变初始迁移标记：{relative}")
        if not rejects_down_with_roll_forward(source_text):
            errors.append(
                f"generated migration 必须拒绝 down、说明追加修复且不得执行其他逻辑：{relative}"
            )
    return errors


def check(
    root: Path, *, require_frozen: bool = False, trusted_ref: str = "HEAD"
) -> list[str]:
    lock_path = root / LOCK_RELATIVE_PATH
    document, entries, load_errors = load_lock(root, lock_path)
    lock_errors = verify_lock(root, lock_path) if not load_errors else load_errors
    locked_paths = {entry.relative for entry in entries}
    try:
        head_paths = git_head_paths(root, trusted_ref)
    except RuntimeError as error:
        return [*lock_errors, str(error)]
    trusted_errors = (
        verify_trusted_head_lock(root, document, entries, head_paths, trusted_ref)
        if document is not None and not load_errors
        else []
    )
    return lock_errors + trusted_errors + verify_generated_registry_wiring(root) + verify_append_only(
        root,
        locked_paths,
        require_frozen=require_frozen,
        head_paths=head_paths,
    )


def render_lock(document: dict[str, object], entries: list[LockedFile]) -> bytes:
    frozen_at = document.get("frozen_at")
    if not isinstance(frozen_at, str) or not frozen_at:
        raise ValueError("迁移冻结清单 frozen_at 必须是非空字符串")
    lines = ["format_version = 1", f'frozen_at = "{frozen_at}"', ""]
    for entry in entries:
        lines.extend(
            [
                "[[files]]",
                f'path = "{entry.relative}"',
                f'sha256 = "{entry.expected_sha256}"',
                f'storage = "{entry.storage}"',
                f'target = "{entry.target}"',
                "",
            ]
        )
    return "\n".join(lines).encode("utf-8")


def write_lock_atomically(lock_path: Path, expected: bytes, content: bytes) -> None:
    staged = lock_path.with_name(f".{lock_path.name}.freeze-{os.getpid()}")
    try:
        with staged.open("xb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        if lock_path.read_bytes() != expected:
            raise RuntimeError("写入前迁移冻结清单发生变化，拒绝覆盖")
        os.replace(staged, lock_path)
    finally:
        staged.unlink(missing_ok=True)


def freeze(root: Path) -> list[str]:
    lock_path = root / LOCK_RELATIVE_PATH
    document, entries, load_errors = load_lock(root, lock_path)
    if load_errors or document is None:
        return load_errors
    errors = verify_lock(root, lock_path)
    errors.extend(verify_generated_registry_wiring(root))
    locked_paths = {entry.relative for entry in entries}
    try:
        head_paths = git_head_paths(root)
    except RuntimeError as error:
        return [str(error)]
    errors.extend(verify_trusted_head_lock(root, document, entries, head_paths))
    errors.extend(
        verify_append_only(
            root,
            locked_paths,
            require_frozen=False,
            head_paths=head_paths,
        )
    )
    if errors:
        return errors

    additions: list[LockedFile] = []
    for migration in discover(root):
        if migration.name in {CONTROL_BASELINE, TENANT_BASELINE}:
            continue
        relative_paths = migration_relative_paths(root, migration)
        if all(relative in locked_paths for relative in relative_paths):
            continue
        source = migration.source.read_text(encoding="utf-8")
        if "尚未实现" in source:
            errors.append(f"迁移仍是未实现骨架，不能冻结：{migration.name}")
            continue
        for path, relative in zip(migration.source_files(), relative_paths, strict=True):
            if relative in locked_paths:
                continue
            if relative in head_paths:
                errors.append(f"拒绝补录已存在于 HEAD 的历史迁移：{relative}")
                continue
            storage, target = lock_identity(relative)
            additions.append(LockedFile(relative, path, sha256(path), storage, target))
    for path in discover_generated(root):
        relative = path.relative_to(root).as_posix()
        if relative in locked_paths:
            continue
        source = path.read_text(encoding="utf-8")
        if "INITIAL_RESOURCE_MIGRATION" not in source or "尚未实现" in source:
            errors.append(f"资源初始迁移不完整，不能冻结：{relative}")
            continue
        if relative in head_paths:
            errors.append(f"拒绝补录已存在于 HEAD 的历史迁移：{relative}")
            continue
        storage, target = lock_identity(relative)
        additions.append(LockedFile(relative, path, sha256(path), storage, target))
    if errors:
        return errors
    if not additions:
        print("没有待冻结的新迁移。")
        return []

    additions.sort(key=lambda item: item.relative)
    before = lock_path.read_bytes()
    write_lock_atomically(lock_path, before, render_lock(document, entries + additions))
    print("已冻结以下新迁移源码：")
    for entry in sorted(additions, key=lambda item: item.relative):
        print(f"  - {entry.relative}")
    return []


def main() -> int:
    arguments = sys.argv[1:]
    trusted_ref = "HEAD"
    trusted_ref_supplied = False
    mode_arguments: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--trusted-ref":
            if trusted_ref_supplied or index + 1 >= len(arguments):
                print("--trusted-ref 必须且只能提供一次 ref/SHA", file=sys.stderr)
                return 2
            trusted_ref = arguments[index + 1]
            trusted_ref_supplied = True
            index += 2
            continue
        mode_arguments.append(argument)
        index += 1
    root = Path(__file__).resolve().parents[1]
    if mode_arguments == ["--freeze"] and not trusted_ref_supplied:
        errors = freeze(root)
        success = "迁移冻结完成；提交后该源码只能通过追加迁移修复。"
    elif mode_arguments == ["--require-frozen"]:
        errors = check(root, require_frozen=True, trusted_ref=trusted_ref)
        success = "迁移历史校验通过：所有待提交迁移已冻结，历史仅允许追加和 roll-forward。"
    elif not mode_arguments:
        errors = check(root, trusted_ref=trusted_ref)
        success = "迁移历史校验通过：冻结迁移未变化，工作树新迁移可继续编辑。"
    else:
        print(
            "用法：check_migration_history.py [--freeze|--require-frozen] "
            "[--trusted-ref <ref-or-sha>]；--freeze 仅使用本地 HEAD",
            file=sys.stderr,
        )
        return 2
    if errors:
        print("迁移历史校验失败：", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print(success)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

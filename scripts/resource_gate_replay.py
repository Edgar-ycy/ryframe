#!/usr/bin/env python3
"""在隔离 Git worktree 中比较 resource targeted 与 full gate 的通过/失败结果。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import stat
import subprocess
import sys
import time
import uuid
from collections.abc import Callable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


SCRIPT_PATH = Path(__file__).resolve()
RUNNER_ROOT = SCRIPT_PATH.parents[1]
FORMAT_VERSION = 2
MINIMUM_CASES = 20
MINIMUM_SUCCESSFUL_CASES = 10
TARGETED_P95_LIMIT_MS = 60_000
TARGETED_ACTIVATION = "replay-verified-v1"
DECISION_FORMAT_VERSION = 1
FIXED_VERIFY_JOBS = "12"
FIXED_TEST_JOBS = "4"
FIXED_RESOURCE_GATE_TEST_JOBS = "12"
SHA_PATTERN = re.compile(r"[0-9a-fA-F]{40}")
REQUIRED_CATEGORIES = frozenset(
    {"addition", "field", "permission", "relation", "sql", "rename", "delete"}
)
TARGETED_MODES = frozenset({"targeted", "full"})
RUST_ENVIRONMENT_KEYS = frozenset(
    {
        "CARGO_BUILD_JOBS",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CARGO_TARGET_DIR",
        "RUSTC",
        "RUSTC_BOOTSTRAP",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
        "RUST_BACKTRACE",
        "RUST_LIB_BACKTRACE",
        "RUST_LOG",
        "RUST_MIN_STACK",
        "RUST_TEST_THREADS",
        "RUSTDOC",
        "RUSTDOCFLAGS",
        "RUSTFLAGS",
        "RUSTUP_TOOLCHAIN",
    }
)


class ReplayConfigurationError(ValueError):
    """重放清单不完整或不安全。"""


@dataclass(frozen=True)
class ReplayCase:
    name: str
    category: str
    base: str
    head: str
    frontend_base: str
    frontend_head: str
    targeted_mode: str
    expected: str


@dataclass(frozen=True)
class ReplayManifest:
    targeted_command: tuple[str, ...]
    full_command: tuple[str, ...]
    cases: tuple[ReplayCase, ...]


@dataclass(frozen=True)
class CommandResult:
    passed: bool
    return_code: int
    duration_ms: int
    order: int
    decision: ResourceGateDecision
    target_fingerprint: str
    output_fingerprint: str
    failure_tail: str | None


@dataclass(frozen=True)
class ResourceGateDecision:
    recognized: bool
    mode: str
    fallback: str | None
    steps: tuple[str, ...]


@dataclass(frozen=True)
class ReplayResult:
    name: str
    category: str
    base: str
    head: str
    frontend_base: str
    frontend_head: str
    targeted_mode: str
    expected: str
    targeted: CommandResult
    full: CommandResult
    matches: bool


@dataclass(frozen=True)
class RepositoryIdentity:
    root: Path
    common_dir: Path
    remote: str | None


@dataclass(frozen=True)
class ReplayTools:
    corepack: str
    sccache: str
    versions: dict[str, str]


@dataclass(frozen=True)
class ReplayEvidence:
    manifest_fingerprint: str
    runner_fingerprint: str
    runner_commit: str
    environment: dict[str, str]
    environment_fingerprint: str
    tools: dict[str, str]
    tools_fingerprint: str
    repositories: dict[str, dict[str, str | None]]
    prime_case: str
    prime_change_count: int
    prime: CommandResult
    sccache_before: dict[str, Any]
    sccache_after: dict[str, Any]


@dataclass(frozen=True)
class ReplayRun:
    results: list[ReplayResult]
    evidence: ReplayEvidence


def load_manifest(path: Path) -> ReplayManifest:
    try:
        raw: Any = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ReplayConfigurationError(f"无法读取 replay manifest：{error}") from error
    if not isinstance(raw, dict):
        raise ReplayConfigurationError("replay manifest 必须是 JSON 对象")
    require_exact_keys(
        raw, {"formatVersion", "targetedCommand", "fullCommand", "cases"}, "manifest"
    )
    if raw["formatVersion"] != FORMAT_VERSION:
        raise ReplayConfigurationError(
            f"replay manifest formatVersion 必须为 {FORMAT_VERSION}"
        )
    targeted = parse_command(raw["targetedCommand"], "targetedCommand")
    full = parse_command(raw["fullCommand"], "fullCommand")
    cases_raw = raw["cases"]
    if not isinstance(cases_raw, list):
        raise ReplayConfigurationError("replay manifest.cases 必须是数组")
    cases = tuple(parse_case(item, index) for index, item in enumerate(cases_raw))
    validate_case_coverage(cases)
    return ReplayManifest(targeted, full, cases)


def require_exact_keys(raw: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(raw)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise ReplayConfigurationError(
            f"{label} 字段不匹配，缺少={missing or '[]'}，多余={extra or '[]'}"
        )


def parse_command(raw: Any, label: str) -> tuple[str, ...]:
    if (
        not isinstance(raw, list)
        or not raw
        or any(not isinstance(item, str) or not item for item in raw)
    ):
        raise ReplayConfigurationError(f"{label} 必须是非空字符串参数数组")
    return tuple(raw)


def parse_case(raw: Any, index: int) -> ReplayCase:
    label = f"cases[{index}]"
    if not isinstance(raw, dict):
        raise ReplayConfigurationError(f"{label} 必须是对象")
    require_exact_keys(
        raw,
        {
            "name",
            "category",
            "base",
            "head",
            "frontendBase",
            "frontendHead",
            "targetedMode",
            "expected",
        },
        label,
    )
    for key in ("name", "category"):
        if not isinstance(raw[key], str) or not raw[key].strip():
            raise ReplayConfigurationError(f"{label}.{key} 必须是非空字符串")
    for key in ("base", "head", "frontendBase", "frontendHead"):
        if not isinstance(raw[key], str) or SHA_PATTERN.fullmatch(raw[key]) is None:
            raise ReplayConfigurationError(f"{label}.{key} 必须是 40 位 Git SHA")
    if raw["expected"] not in ("pass", "fail"):
        raise ReplayConfigurationError(f"{label}.expected 只允许 pass 或 fail")
    if raw["targetedMode"] not in TARGETED_MODES:
        raise ReplayConfigurationError(f"{label}.targetedMode 只允许 targeted 或 full")
    return ReplayCase(
        name=raw["name"].strip(),
        category=raw["category"].strip(),
        base=raw["base"].lower(),
        head=raw["head"].lower(),
        frontend_base=raw["frontendBase"].lower(),
        frontend_head=raw["frontendHead"].lower(),
        targeted_mode=raw["targetedMode"],
        expected=raw["expected"],
    )


def validate_case_coverage(cases: tuple[ReplayCase, ...]) -> None:
    if len(cases) < MINIMUM_CASES:
        raise ReplayConfigurationError(f"replay 至少需要 {MINIMUM_CASES} 个案例")
    names = [case.name for case in cases]
    if len(set(names)) != len(names):
        raise ReplayConfigurationError("replay 案例名称必须唯一")
    ranges = [(case.base, case.head) for case in cases]
    if len(set(ranges)) != len(ranges):
        raise ReplayConfigurationError("replay 案例的 base/head 范围必须唯一")
    categories = {case.category for case in cases}
    missing = sorted(REQUIRED_CATEGORIES - categories)
    if missing:
        raise ReplayConfigurationError(f"replay 缺少类别：{', '.join(missing)}")
    unchanged = [case.name for case in cases if case.base == case.head]
    if unchanged:
        raise ReplayConfigurationError(
            "replay 案例 base/head 不得相同：" + ", ".join(unchanged)
        )
    outcomes = {case.expected for case in cases}
    if outcomes != {"pass", "fail"}:
        raise ReplayConfigurationError("replay 必须同时覆盖预期 pass 与 fail 案例")
    modes = {case.targeted_mode for case in cases}
    if modes != TARGETED_MODES:
        raise ReplayConfigurationError("replay 必须同时覆盖 targeted 与 full 模式")
    targeted_passes = sum(
        case.expected == "pass" and case.targeted_mode == "targeted" for case in cases
    )
    if targeted_passes < MINIMUM_SUCCESSFUL_CASES:
        raise ReplayConfigurationError(
            f"replay 至少需要 {MINIMUM_SUCCESSFUL_CASES} 个预期成功的 targeted 案例"
        )


def run_replay(
    repository: Path,
    frontend_repository: Path,
    manifest: ReplayManifest,
    work_root: Path,
    *,
    manifest_fingerprint: str,
    activation_gate: bool,
) -> ReplayRun:
    backend = repository_identity(repository, "后端", "Cargo.toml")
    frontend = repository_identity(frontend_repository, "前端", "package.json")
    validate_repository_pair(backend, frontend)
    if not frontend.root.joinpath("pnpm-lock.yaml").is_file():
        raise ReplayConfigurationError("前端 replay 仓库缺少 pnpm-lock.yaml")
    allowed = (backend.root / ".local-tests" / "resource-gate-replay").resolve()
    work_root = work_root.resolve()
    if not work_root.is_relative_to(allowed):
        raise ReplayConfigurationError(f"replay work-dir 必须位于 {allowed}")
    allowed.mkdir(parents=True, exist_ok=True)
    work_root.mkdir(parents=True, exist_ok=True)
    corepack, sccache = resolve_tool_executables()
    session = (work_root / f"r-{uuid.uuid4().hex[:12]}").resolve()
    if not session.is_relative_to(allowed):
        raise ReplayConfigurationError("replay 临时目录逃逸允许范围")
    shared_target = (session / "target").resolve()
    if not shared_target.is_relative_to(session):
        raise ReplayConfigurationError("replay 共享 Cargo target 逃逸 session")
    shared_worktree = (session / "worktree").resolve()
    if not shared_worktree.is_relative_to(session):
        raise ReplayConfigurationError("replay 共享 worktree 逃逸 session")
    cache = (work_root / "cache" / session.name).resolve()
    if not cache.is_relative_to(allowed):
        raise ReplayConfigurationError("replay sccache 目录逃逸允许范围")
    environment = controlled_environment(sccache, cache, activation_gate)
    tools = collect_tools(backend.root, frontend.root, corepack, sccache, environment)
    session.mkdir()
    try:
        cache.mkdir(parents=True)
    except OSError as error:
        session.rmdir()
        raise ReplayConfigurationError(
            f"无法创建 replay sccache 目录 {cache}：{error}"
        ) from error
    results: list[ReplayResult] = []
    prime: CommandResult | None = None
    before: dict[str, Any] = {}
    after: dict[str, Any] = {}
    global_order = 0
    sccache_started = False
    backend_worktree_created = False
    frontend_worktree_created = False
    primary_error: BaseException | None = None
    try:
        ensure_sccache_server(tools.sccache, environment)
        sccache_started = True
        prime_case, prime_change_count = select_prime_case(
            manifest.cases,
            lambda case: replay_case_change_count(backend, frontend, case),
        )
        verify_case_ranges(backend, frontend, prime_case)
        shared_worktree.mkdir()
        add_worktree(backend.root, shared_worktree / "b", prime_case.head, "后端")
        backend_worktree_created = True
        add_worktree(
            frontend.root,
            shared_worktree / "f",
            prime_case.frontend_head,
            "前端",
        )
        frontend_worktree_created = True
        prime = execute_in_worktree(
            shared_worktree,
            shared_target,
            prime_case,
            manifest.targeted_command,
            environment,
            tools.corepack,
            targeted=True,
            order=0,
        )
        require_arm(prime_case, prime, targeted=True, require_pass=True)
        run_sccache(tools.sccache, environment, "--zero-stats")
        before = sccache_stats(tools.sccache, environment)

        for index, case in enumerate(manifest.cases, start=1):
            verify_case_ranges(backend, frontend, case)
            arms: dict[bool, CommandResult] = {}
            for targeted in arm_order(index):
                global_order += 1
                command = (
                    manifest.targeted_command if targeted else manifest.full_command
                )
                arms[targeted] = execute_in_worktree(
                    shared_worktree,
                    shared_target,
                    case,
                    command,
                    environment,
                    tools.corepack,
                    targeted=targeted,
                    order=global_order,
                )
                require_arm(case, arms[targeted], targeted=targeted)
            targeted = arms[True]
            full = arms[False]
            expected_pass = case.expected == "pass"
            results.append(
                ReplayResult(
                    name=case.name,
                    category=case.category,
                    base=case.base,
                    head=case.head,
                    frontend_base=case.frontend_base,
                    frontend_head=case.frontend_head,
                    targeted_mode=case.targeted_mode,
                    expected=case.expected,
                    targeted=targeted,
                    full=full,
                    matches=(
                        targeted.passed == expected_pass
                        and full.passed == expected_pass
                        and targeted.decision.mode == case.targeted_mode
                        and full.decision.mode == "full"
                    ),
                )
            )
        after = sccache_stats(tools.sccache, environment)
        safe_environment = recorded_environment(environment, activation_gate)
        evidence = ReplayEvidence(
            manifest_fingerprint=manifest_fingerprint,
            runner_fingerprint=sha256_file(SCRIPT_PATH),
            runner_commit=git_output(RUNNER_ROOT, "rev-parse", "HEAD"),
            environment=safe_environment,
            environment_fingerprint=sha256_json(safe_environment),
            tools=tools.versions,
            tools_fingerprint=sha256_json(tools.versions),
            repositories={
                "backend": repository_record(backend),
                "frontend": repository_record(frontend),
            },
            prime_case=prime_case.name,
            prime_change_count=prime_change_count,
            prime=prime,
            sccache_before=before,
            sccache_after=after,
        )
        return ReplayRun(results, evidence)
    except BaseException as error:
        primary_error = error
        raise
    finally:
        cleanup_errors: list[str] = []
        if sccache_started:
            error = try_sccache_stop(tools.sccache, environment)
            if error is not None:
                cleanup_errors.append(error)
        cleanup_errors.extend(
            cleanup_created_worktrees(
                (
                    (
                        frontend_worktree_created or (shared_worktree / "f").exists(),
                        frontend.root,
                        shared_worktree / "f",
                        "前端",
                    ),
                    (
                        backend_worktree_created or (shared_worktree / "b").exists(),
                        backend.root,
                        shared_worktree / "b",
                        "后端",
                    ),
                ),
                shared_worktree,
            )
        )
        cleanup_errors.extend(prune_worktrees(backend.root, frontend.root))
        cleanup_errors.extend(remove_isolated_tree(cache, allowed))
        cleanup_errors.extend(remove_isolated_tree(shared_target, allowed))
        cleanup_errors.extend(remove_isolated_tree(shared_worktree, allowed))
        if session.is_dir():
            try:
                session.rmdir()
            except OSError as error:
                cleanup_errors.append(f"replay 临时目录未完全回收：{session}：{error}")
        fail_on_cleanup(primary_error, "replay 清理", cleanup_errors)


def command_for_frontend(command: tuple[str, ...], frontend: Path) -> tuple[str, ...]:
    normalized = list(command)
    try:
        option = normalized.index("--frontend-dir")
    except ValueError:
        normalized.extend(("--frontend-dir", str(frontend)))
        return tuple(normalized)
    if option + 1 >= len(normalized):
        raise ReplayConfigurationError("--frontend-dir 缺少路径参数")
    normalized[option + 1] = str(frontend)
    return tuple(normalized)


def command_for_replay(
    command: tuple[str, ...], frontend: Path, runner_target: Path, workspace_root: Path
) -> tuple[str, ...]:
    """在回放中展开 cargo xtask 别名，避免别名的深层相对 target。"""
    normalized = list(command_for_frontend(command, frontend))
    if len(normalized) >= 2 and normalized[0:2] == ["cargo", "xtask"]:
        return (
            "cargo",
            "run",
            "--locked",
            "--manifest-path",
            str((RUNNER_ROOT / "Cargo.toml").resolve()),
            "--config",
            f'env.RYFRAME_WORKSPACE_ROOT="{workspace_root.as_posix()}"',
            "--target-dir",
            str(runner_target.resolve()),
            "-p",
            "xtask",
            "--",
            *normalized[2:],
        )
    return tuple(normalized)


def arm_order(index: int) -> tuple[bool, bool]:
    if index < 1:
        raise ValueError("replay case index 必须从 1 开始")
    return (True, False) if index % 2 == 1 else (False, True)


def verify_commit_range(
    repository: Path,
    base: str,
    head: str,
    case_name: str,
    label: str,
    *,
    allow_unchanged: bool = False,
) -> None:
    if not allow_unchanged and base == head:
        raise ReplayConfigurationError(f"replay 案例 {case_name} 的{label}范围没有变化")
    for commit in (base, head):
        completed = subprocess.run(
            ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
            cwd=repository,
            check=False,
            capture_output=True,
        )
        if completed.returncode != 0:
            raise ReplayConfigurationError(f"{label} fixture commit 不存在：{commit}")
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", base, head],
        cwd=repository,
        check=False,
        capture_output=True,
    )
    if ancestor.returncode != 0:
        raise ReplayConfigurationError(
            f"replay 案例 {case_name} 的{label} base 不是 head 祖先"
        )


def verify_case_ranges(
    backend: RepositoryIdentity,
    frontend: RepositoryIdentity,
    case: ReplayCase,
) -> None:
    verify_commit_range(backend.root, case.base, case.head, case.name, "后端")
    verify_commit_range(
        frontend.root,
        case.frontend_base,
        case.frontend_head,
        case.name,
        "前端",
        allow_unchanged=True,
    )


def select_prime_case(
    cases: tuple[ReplayCase, ...],
    change_count: Callable[[ReplayCase], int],
) -> tuple[ReplayCase, int]:
    """选择变更面最大的成功定向案例，为暖缓存回放建立完整编译面。"""
    candidates = tuple(
        case
        for case in cases
        if case.expected == "pass" and case.targeted_mode == "targeted"
    )
    if not candidates:
        raise ReplayConfigurationError("replay 缺少可用于预热的成功定向案例")
    ranked = tuple((case, change_count(case)) for case in candidates)
    if any(count < 0 for _, count in ranked):
        raise ReplayConfigurationError("replay 预热案例的变更文件数不得为负数")
    return max(
        enumerate(ranked),
        key=lambda item: (item[1][1], -item[0]),
    )[1]


def replay_case_change_count(
    backend: RepositoryIdentity,
    frontend: RepositoryIdentity,
    case: ReplayCase,
) -> int:
    return changed_file_count(backend.root, case.base, case.head) + changed_file_count(
        frontend.root, case.frontend_base, case.frontend_head
    )


def changed_file_count(repository: Path, base: str, head: str) -> int:
    if base == head:
        return 0
    output = git_output(repository, "diff", "--name-only", "--no-renames", base, head)
    return sum(1 for line in output.splitlines() if line.strip())


def repository_identity(path: Path, label: str, marker: str) -> RepositoryIdentity:
    root = path.resolve()
    if not root.is_dir():
        raise ReplayConfigurationError(f"{label} replay 仓库目录不存在：{root}")
    top_level = Path(git_output(root, "rev-parse", "--show-toplevel")).resolve()
    if not same_path(root, top_level):
        raise ReplayConfigurationError(
            f"{label} --repository 必须指向 Git 仓库顶层：{top_level}"
        )
    if not root.joinpath(marker).is_file():
        raise ReplayConfigurationError(f"{label} replay 仓库缺少 {marker}")
    common_value = Path(git_output(root, "rev-parse", "--git-common-dir"))
    common_dir = (
        common_value if common_value.is_absolute() else root.joinpath(common_value)
    ).resolve()
    remote_result = subprocess.run(
        ["git", "config", "--get", "remote.origin.url"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    remote = remote_result.stdout.strip() if remote_result.returncode == 0 else ""
    return RepositoryIdentity(root, common_dir, sha256_text(remote) if remote else None)


def same_path(left: Path, right: Path) -> bool:
    return os.path.normcase(str(left.resolve())) == os.path.normcase(
        str(right.resolve())
    )


def validate_repository_pair(
    backend: RepositoryIdentity, frontend: RepositoryIdentity
) -> None:
    if same_path(backend.common_dir, frontend.common_dir):
        raise ReplayConfigurationError(
            "前后端 replay 必须使用 git common-dir 不同的独立仓库"
        )


def git_output(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            f"Git 命令失败（{' '.join(arguments)}）：{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def resolve_tool_executables() -> tuple[str, str]:
    corepack = required_executable("corepack")
    sccache = required_executable("sccache")
    return corepack, sccache


def collect_tools(
    backend: Path,
    frontend: Path,
    corepack: str,
    sccache: str,
    environment: dict[str, str],
) -> ReplayTools:
    versions = {
        "python": sys.version.splitlines()[0],
        "git": tool_version(backend, "git", environment, "--version"),
        "cargo": tool_version(backend, "cargo", environment, "--version"),
        "rustc": tool_version(backend, "rustc", environment, "-vV"),
        "node": tool_version(frontend, "node", environment, "--version"),
        "corepack": tool_version(frontend, corepack, environment, "--version"),
        "pnpm": tool_version(frontend, corepack, environment, "pnpm", "--version"),
        "sccache": tool_version(backend, sccache, environment, "--version"),
    }
    return ReplayTools(corepack, sccache, versions)


def required_executable(name: str) -> str:
    resolved = shutil.which(name)
    if resolved is None:
        raise ReplayConfigurationError(f"resource replay 缺少可执行文件：{name}")
    return str(Path(resolved).resolve())


def tool_version(
    root: Path,
    program: str,
    environment: dict[str, str],
    *arguments: str,
) -> str:
    completed = subprocess.run(
        [program, *arguments],
        cwd=root,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            f"无法读取工具版本 {program}：{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def controlled_environment(
    sccache: str,
    cache: Path,
    activation_gate: bool,
) -> dict[str, str]:
    environment = os.environ.copy()
    if activation_gate:
        for name in (
            "RYFRAME_MYSQL_INTEGRATION",
            "RYFRAME_MYSQL_TLS_INTEGRATION",
            "RYFRAME_REDIS_INTEGRATION",
        ):
            if environment.get(name) != "1":
                raise ReplayConfigurationError(
                    f"activation replay 要求 {name}=1 并准备隔离服务"
                )
        for name in ("RYFRAME_MYSQL_HOST", "RYFRAME_REDIS_HOST"):
            if environment.get(name) not in {"127.0.0.1", "localhost", "::1"}:
                raise ReplayConfigurationError(
                    f"activation replay 要求 {name} 显式使用回环地址"
                )
        if environment.get("RYFRAME_REDIS_DATABASE") != "15":
            raise ReplayConfigurationError(
                "activation replay 要求 RYFRAME_REDIS_DATABASE=15"
            )
    for key in tuple(environment):
        if (
            key.startswith("RYFRAME_CI_")
            or key.startswith("SCCACHE_")
            or key in RUST_ENVIRONMENT_KEYS
            or key
            in {
                "RYFRAME_DEVEX_TARGET_ROOT",
                "RYFRAME_RESOURCE_GATE_DECISION_FILE",
                "RYFRAME_RESOURCE_GATE_TEST_JOBS",
                "RYFRAME_RESOURCE_GATE_TARGETED",
                "RYFRAME_VERIFY_JOBS",
                "CMAKE_C_COMPILER_LAUNCHER",
                "CMAKE_CXX_COMPILER_LAUNCHER",
            }
        ):
            environment.pop(key, None)
    environment.update(
        {
            "CI": "1",
            "CARGO_INCREMENTAL": "0",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": sccache,
            "RYFRAME_CI_RUST_GATE_PROFILE": "standard",
            "RYFRAME_CI_TEST_JOBS": FIXED_TEST_JOBS,
            "RYFRAME_RESOURCE_GATE_TEST_JOBS": FIXED_RESOURCE_GATE_TEST_JOBS,
            "RYFRAME_VERIFY_JOBS": FIXED_VERIFY_JOBS,
            "SCCACHE_DIR": str(cache),
            "SCCACHE_SERVER_PORT": str(available_port()),
        }
    )
    return environment


def available_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def recorded_environment(
    environment: dict[str, str], activation_gate: bool
) -> dict[str, str]:
    recorded = {
        key: environment[key]
        for key in (
            "CI",
            "CARGO_INCREMENTAL",
            "CARGO_TERM_COLOR",
            "RYFRAME_CI_RUST_GATE_PROFILE",
            "RYFRAME_CI_TEST_JOBS",
            "RYFRAME_RESOURCE_GATE_TEST_JOBS",
            "RYFRAME_VERIFY_JOBS",
        )
    }
    recorded.update(
        {
            "RUSTC_WRAPPER": "sccache",
            "SCCACHE_BACKEND": "dedicated-local",
            "activationGate": str(activation_gate).lower(),
            "frontendInstall": "corepack pnpm install --offline --frozen-lockfile",
            "mysqlIntegration": environment.get("RYFRAME_MYSQL_INTEGRATION", "0"),
            "mysqlTlsIntegration": environment.get(
                "RYFRAME_MYSQL_TLS_INTEGRATION", "0"
            ),
            "redisIntegration": environment.get("RYFRAME_REDIS_INTEGRATION", "0"),
        }
    )
    basedirs = tuple(
        part
        for part in environment.get("SCCACHE_BASEDIRS", "").split(os.pathsep)
        if part
    )
    if basedirs:
        recorded.update(
            {
                "SCCACHE_BASEDIRS_COUNT": str(len(basedirs)),
                "SCCACHE_BASEDIRS_FINGERPRINT": sha256_json(
                    [os.path.normcase(str(Path(path).resolve())) for path in basedirs]
                ),
            }
        )
    return recorded


def run_sccache(program: str, environment: dict[str, str], argument: str) -> None:
    completed = subprocess.run(
        [program, argument],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            f"sccache {argument} 失败：{completed.stderr.strip()}"
        )


def ensure_sccache_server(program: str, environment: dict[str, str]) -> None:
    """启动并确认 sccache；兼容 Windows 本机启动命令的迟到返回。"""
    try:
        run_sccache(program, environment, "--start-server")
        return
    except ReplayConfigurationError as error:
        # 某些 Windows 版本的 sccache 在服务已进入监听后仍会返回启动超时。
        # 只有统计接口成功时才接受该状态，其他错误继续按配置错误处理。
        if "Timed out waiting for server startup" not in str(error):
            raise
        try:
            sccache_stats(program, environment)
        except ReplayConfigurationError:
            raise error


def sccache_stats(program: str, environment: dict[str, str]) -> dict[str, Any]:
    completed = subprocess.run(
        [program, "--show-stats", "--stats-format", "json"],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(f"sccache 统计失败：{completed.stderr.strip()}")
    try:
        document = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise ReplayConfigurationError(f"sccache 统计不是合法 JSON：{error}") from error
    if not isinstance(document, dict):
        raise ReplayConfigurationError("sccache 统计必须是 JSON 对象")
    return document


def sccache_error_count(document: dict[str, Any]) -> int:
    stats = document.get("stats")
    if not isinstance(stats, dict):
        raise ReplayConfigurationError("sccache 统计缺少 stats 对象")
    errors = count_map(stats.get("cache_errors"), "stats.cache_errors")
    for name in (
        "cache_timeouts",
        "cache_read_errors",
        "cache_write_errors",
        "dist_errors",
    ):
        errors += non_negative_int(stats.get(name), f"stats.{name}")
    return errors


def count_map(value: Any, label: str) -> int:
    if not isinstance(value, dict) or not isinstance(value.get("counts"), dict):
        raise ReplayConfigurationError(f"sccache {label}.counts 必须是对象")
    return sum(
        non_negative_int(count, f"{label}.counts.{name}")
        for name, count in value["counts"].items()
    )


def non_negative_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ReplayConfigurationError(f"sccache {label} 必须是非负整数")
    return value


def try_sccache_stop(program: str, environment: dict[str, str]) -> str | None:
    try:
        run_sccache(program, environment, "--stop-server")
    except ReplayConfigurationError as error:
        return str(error)
    return None


def execute_in_worktree(
    worktree: Path,
    shared_target: Path,
    case: ReplayCase,
    command: tuple[str, ...],
    base_environment: dict[str, str],
    corepack: str,
    *,
    targeted: bool,
    order: int,
) -> CommandResult:
    backend_worktree = worktree / "b"
    frontend_worktree = worktree / "f"
    decision_path = worktree / "decision.json"
    command_log = worktree / "command.log"
    primary_error: BaseException | None = None
    try:
        switch_worktree(backend_worktree, case.head, "后端")
        switch_worktree(frontend_worktree, case.frontend_head, "前端")
        install_frontend_dependencies(frontend_worktree, corepack, base_environment)
        target_root = shared_target.resolve()
        if not target_root.is_relative_to(worktree.parent.resolve()):
            raise ReplayConfigurationError("replay Cargo target 逃逸 session")
        environment = base_environment.copy()
        environment.update(
            {
                "RYFRAME_CI_BASE_SHA": case.base,
                "RYFRAME_CI_HEAD_SHA": case.head,
                "RYFRAME_CI_FRONTEND_REF": case.frontend_head,
                "RYFRAME_DEVEX_TARGET_ROOT": str(target_root),
                "RYFRAME_WORKSPACE_ROOT": str(backend_worktree.resolve()),
                # 外层 xtask 也固定到 session 级短路径，避免 worktree 深路径
                # 触发 Windows MAX_PATH，并复用各案例之间的 xtask 编译产物。
                "CARGO_TARGET_DIR": str((target_root / "runner").resolve()),
                "RYFRAME_INTEGRATION_RUN_ID": f"resource-replay-{worktree.name}",
                "RYFRAME_RESOURCE_GATE_DECISION_FILE": str(decision_path.resolve()),
            }
        )
        if targeted:
            environment["RYFRAME_RESOURCE_GATE_TARGETED"] = TARGETED_ACTIVATION
        else:
            environment.pop("RYFRAME_RESOURCE_GATE_TARGETED", None)
        started = time.perf_counter_ns()
        with command_log.open("wb") as output:
            completed = subprocess.run(
                list(
                    command_for_replay(
                        command,
                        frontend_worktree,
                        target_root / "runner",
                        backend_worktree,
                    )
                ),
                cwd=backend_worktree,
                env=environment,
                check=False,
                stdout=output,
                stderr=subprocess.STDOUT,
            )
        duration_ms = (time.perf_counter_ns() - started) // 1_000_000
        decision = load_decision_after_command(
            decision_path, command_log, completed.returncode
        )
        return CommandResult(
            passed=completed.returncode == 0,
            return_code=completed.returncode,
            duration_ms=duration_ms,
            order=order,
            decision=decision,
            target_fingerprint=sha256_text(str(target_root)),
            output_fingerprint=sha256_file(command_log),
            failure_tail=(
                bounded_log_tail(command_log) if completed.returncode != 0 else None
            ),
        )
    except BaseException as error:
        primary_error = error
        raise
    finally:
        cleanup_errors: list[str] = []
        if decision_path.exists():
            try:
                decision_path.unlink()
            except OSError as error:
                cleanup_errors.append(f"无法删除 decision artifact：{error}")
        if command_log.exists():
            try:
                command_log.unlink()
            except OSError as error:
                cleanup_errors.append(f"无法删除命令日志：{error}")
        fail_on_cleanup(primary_error, "案例清理", cleanup_errors)


def add_worktree(repository: Path, worktree: Path, commit: str, label: str) -> None:
    completed = subprocess.run(
        ["git", "worktree", "add", "--quiet", "--detach", str(worktree), commit],
        cwd=repository,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            f"无法创建{label} replay worktree：{completed.stderr.strip()}"
        )


def switch_worktree(worktree: Path, commit: str, label: str) -> None:
    current = git_output(worktree, "rev-parse", "HEAD")
    if current.casefold() == commit.casefold():
        return
    completed = subprocess.run(
        ["git", "checkout", "--quiet", "--detach", commit],
        cwd=worktree,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            f"无法切换{label} replay worktree：{completed.stderr.strip()}"
        )


def install_frontend_dependencies(
    frontend: Path,
    corepack: str,
    environment: dict[str, str],
) -> None:
    fingerprint = frontend_dependency_fingerprint(frontend)
    marker = frontend / "node_modules" / ".ryframe-resource-replay-dependencies"
    try:
        if marker.read_text(encoding="utf-8").strip() == fingerprint:
            return
    except (OSError, UnicodeError):
        pass
    completed = subprocess.run(
        [corepack, "pnpm", "install", "--offline", "--frozen-lockfile"],
        cwd=frontend,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise ReplayConfigurationError(
            "前端 replay 依赖离线安装失败；请先预热 pnpm store："
            + completed.stderr.strip()
        )
    if not frontend.joinpath("node_modules").is_dir():
        raise ReplayConfigurationError("前端 replay 离线安装后仍缺少 node_modules")
    try:
        marker.write_text(fingerprint + "\n", encoding="utf-8")
    except OSError as error:
        raise ReplayConfigurationError(
            f"无法写入前端 replay 依赖指纹：{error}"
        ) from error


def frontend_dependency_fingerprint(frontend: Path) -> str:
    digest = hashlib.sha256()
    files = [
        frontend / name
        for name in ("package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml", ".npmrc")
        if (frontend / name).is_file()
    ]
    patches = frontend / "patches"
    if patches.is_dir():
        files.extend(path for path in patches.rglob("*") if path.is_file())
    for path in sorted(files, key=lambda value: value.relative_to(frontend).as_posix()):
        relative = path.relative_to(frontend).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        body = path.read_bytes()
        digest.update(len(body).to_bytes(8, "big"))
        digest.update(body)
    return "sha256:" + digest.hexdigest()


def load_decision(path: Path) -> ResourceGateDecision:
    try:
        raw: Any = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ReplayConfigurationError(
            f"无法读取 resource gate decision：{error}"
        ) from error
    if not isinstance(raw, dict):
        raise ReplayConfigurationError("resource gate decision 必须是 JSON 对象")
    require_exact_keys(
        raw, {"formatVersion", "recognized", "mode", "fallback", "steps"}, "decision"
    )
    if raw["formatVersion"] != DECISION_FORMAT_VERSION:
        raise ReplayConfigurationError(
            f"resource gate decision formatVersion 必须为 {DECISION_FORMAT_VERSION}"
        )
    if not isinstance(raw["recognized"], bool):
        raise ReplayConfigurationError("resource gate decision.recognized 必须是布尔值")
    if raw["mode"] not in TARGETED_MODES:
        raise ReplayConfigurationError("resource gate decision.mode 非法")
    fallback = raw["fallback"]
    if fallback is not None and (not isinstance(fallback, str) or not fallback.strip()):
        raise ReplayConfigurationError(
            "resource gate decision.fallback 必须为空或非空字符串"
        )
    steps = raw["steps"]
    if (
        not isinstance(steps, list)
        or not steps
        or any(not isinstance(step, str) or not step for step in steps)
    ):
        raise ReplayConfigurationError(
            "resource gate decision.steps 必须是非空字符串数组"
        )
    if raw["mode"] == "targeted" and fallback is not None:
        raise ReplayConfigurationError("targeted decision 不得包含 fallback")
    if raw["mode"] == "full" and fallback is None:
        raise ReplayConfigurationError("full decision 必须包含 fallback")
    return ResourceGateDecision(raw["recognized"], raw["mode"], fallback, tuple(steps))


def load_decision_after_command(
    decision_path: Path, command_log: Path, return_code: int
) -> ResourceGateDecision:
    try:
        return load_decision(decision_path)
    except ReplayConfigurationError as error:
        tail = bounded_log_tail(command_log)
        detail = f"\n命令日志末尾：\n{tail}" if tail else ""
        raise ReplayConfigurationError(
            f"{error}；命令退出码={return_code}{detail}"
        ) from error


def require_arm(
    case: ReplayCase,
    result: CommandResult,
    *,
    targeted: bool,
    require_pass: bool = False,
) -> None:
    if targeted:
        if not result.decision.recognized:
            raise ReplayConfigurationError(
                f"replay {case.name} 的 targeted arm 未识别激活标记"
            )
        if result.decision.mode != case.targeted_mode:
            raise ReplayConfigurationError(
                f"replay {case.name} 的 targeted mode 应为 {case.targeted_mode}，"
                f"实际为 {result.decision.mode}"
            )
    elif result.decision.recognized or result.decision.mode != "full":
        raise ReplayConfigurationError(
            f"replay {case.name} 的 full arm 未执行未激活的完整门禁"
        )
    if require_pass and not result.passed:
        detail = (
            f"\n失败日志末尾：\n{result.failure_tail}" if result.failure_tail else ""
        )
        raise ReplayConfigurationError(f"replay prime 案例失败：{case.name}{detail}")


def bounded_log_tail(path: Path, limit: int = 8192) -> str:
    if limit < 1:
        raise ValueError("日志末尾上限必须大于 0")
    with path.open("rb") as source:
        source.seek(0, os.SEEK_END)
        length = source.tell()
        source.seek(max(0, length - limit))
        body = source.read(limit)
    return body.decode("utf-8", errors="replace").replace("\x00", "").strip()


def cleanup_worktree(
    repository: Path,
    worktree: Path,
    label: str,
    allowed: Path,
) -> str | None:
    attempts = 5 if os.name == "nt" else 1
    last_error = ""
    for attempt in range(attempts):
        cleanup = subprocess.run(
            ["git", "worktree", "remove", "--force", str(worktree)],
            cwd=repository,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if cleanup.returncode == 0:
            return None
        last_error = cleanup.stderr.strip() or str(cleanup.returncode)
        if os.name == "nt" and attempt + 1 < attempts:
            time.sleep(0.1 * (2**attempt))
    directory_error = remove_partial_worktree_directory(worktree, allowed)
    if directory_error is not None:
        return (
            f"无法回收{label} replay worktree {worktree}："
            f"{last_error}；{directory_error}"
        )
    prune = subprocess.run(
        ["git", "worktree", "prune"],
        cwd=repository,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if prune.returncode != 0:
        detail = prune.stderr.strip() or str(prune.returncode)
        return f"无法 prune {label} replay worktree：{detail}"
    return None


def remove_partial_worktree_directory(worktree: Path, allowed: Path) -> str | None:
    allowed = allowed.resolve()
    resolved = worktree.resolve()
    if resolved == allowed or not resolved.is_relative_to(allowed):
        return f"拒绝清理允许范围外目录：{resolved}"
    attempts = 5 if os.name == "nt" else 1
    last_error = ""
    for attempt in range(attempts):
        try:
            shutil.rmtree(rmtree_path(resolved), onexc=recover_rmtree_entry)
        except OSError as error:
            last_error = str(error)
        else:
            if not resolved.exists():
                return None
            last_error = "删除调用返回成功后目录仍存在"
        if os.name != "nt" or attempt + 1 == attempts:
            return f"无法删除残留目录 {resolved}：{last_error}"
        time.sleep(0.1 * (2**attempt))
    return f"无法删除残留目录 {resolved}：{last_error}"


def rmtree_path(path: Path) -> Path | str:
    if os.name != "nt":
        return path
    value = str(path)
    if value.startswith("\\\\?\\"):
        return value
    if value.startswith("\\\\"):
        return "\\\\?\\UNC\\" + value[2:]
    return "\\\\?\\" + value


def recover_rmtree_entry(
    function: Callable[[str], object],
    path: str,
    error: BaseException,
) -> None:
    if isinstance(error, FileNotFoundError):
        return
    try:
        os.chmod(path, stat.S_IWRITE)
        function(path)
    except FileNotFoundError:
        return


def cleanup_created_worktrees(
    entries: tuple[tuple[bool, Path, Path, str], ...],
    allowed: Path,
) -> list[str]:
    errors: list[str] = []
    for created, repository, worktree, label in entries:
        if not created:
            continue
        error = cleanup_worktree(repository, worktree, label, allowed)
        if error is not None:
            errors.append(error)
    return errors


def fail_on_cleanup(
    primary: BaseException | None,
    label: str,
    errors: list[str],
) -> None:
    if not errors:
        return
    message = f"{label}失败：" + "；".join(errors)
    if primary is None:
        raise ReplayConfigurationError(message)
    if isinstance(primary, ReplayConfigurationError):
        raise ReplayConfigurationError(f"{primary}；{message}") from primary
    primary.add_note(message)


def prune_worktrees(*repositories: Path) -> list[str]:
    errors: list[str] = []
    for repository in repositories:
        completed = subprocess.run(
            ["git", "worktree", "prune"],
            cwd=repository,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if completed.returncode != 0:
            errors.append(
                f"无法 prune {repository}：{completed.stderr.strip() or completed.returncode}"
            )
    return errors


def remove_isolated_tree(path: Path, allowed: Path) -> list[str]:
    resolved = path.resolve()
    if resolved == allowed or not resolved.is_relative_to(allowed):
        return [f"拒绝清理允许范围外目录：{resolved}"]
    if not resolved.exists():
        return []
    attempts = 5 if os.name == "nt" else 1
    for attempt in range(attempts):
        try:
            shutil.rmtree(resolved)
            try:
                resolved.parent.rmdir()
            except OSError:
                pass
            return []
        except OSError as error:
            if os.name != "nt" or attempt + 1 == attempts:
                return [f"无法清理 replay 隔离目录 {resolved}：{error}"]
            time.sleep(0.1 * (2**attempt))
    return []


def sha256_file(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_text(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()


def sha256_json(value: Any) -> str:
    body = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return sha256_text(body)


def repository_record(identity: RepositoryIdentity) -> dict[str, str | None]:
    return {
        "rootFingerprint": sha256_text(os.path.normcase(str(identity.root))),
        "commonDirFingerprint": sha256_text(os.path.normcase(str(identity.common_dir))),
        "remoteFingerprint": identity.remote,
    }


def write_report(
    path: Path,
    run: ReplayRun,
    *,
    activation_gate: bool = False,
) -> None:
    results = run.results
    zero_divergence = bool(results) and all(result.matches for result in results)
    successful_targeted = [
        result.targeted.duration_ms
        for result in results
        if (
            result.expected == "pass"
            and result.targeted_mode == "targeted"
            and result.targeted.passed
            and result.targeted.decision.mode == "targeted"
        )
    ]
    targeted_p95_ms = percentile_nearest_rank(successful_targeted, 95)
    targeted_within_budget = (
        len(successful_targeted) >= MINIMUM_SUCCESSFUL_CASES
        and targeted_p95_ms is not None
        and targeted_p95_ms <= TARGETED_P95_LIMIT_MS
    )
    replay_coverage_eligible = len(successful_targeted) >= MINIMUM_SUCCESSFUL_CASES
    cache_errors = (
        sccache_error_count(run.evidence.sccache_after) if activation_gate else None
    )
    cache_healthy = cache_errors == 0 if activation_gate else True
    document = {
        "formatVersion": FORMAT_VERSION,
        "caseCount": len(results),
        "successfulTargetedCaseCount": len(successful_targeted),
        "zeroDivergence": zero_divergence,
        "targetedP95Ms": targeted_p95_ms,
        "targetedP95LimitMs": TARGETED_P95_LIMIT_MS,
        "targetedWithinBudget": targeted_within_budget,
        "replayCoverageEligible": replay_coverage_eligible,
        "sccacheCacheErrors": cache_errors,
        "sccacheHealthy": cache_healthy,
        "activationEligible": replay_activation_eligible(
            activation_gate,
            zero_divergence,
            replay_coverage_eligible,
            cache_healthy,
            targeted_within_budget,
        ),
        "evidence": asdict(run.evidence),
        "cases": [asdict(result) for result in results],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def percentile_nearest_rank(samples: list[int], percentile: int) -> int | None:
    if not samples:
        return None
    if not 1 <= percentile <= 100:
        raise ValueError("percentile 必须在 1..=100")
    ordered = sorted(samples)
    rank = (percentile * len(ordered) + 99) // 100
    return ordered[rank - 1]


def replay_activation_eligible(
    activation_gate: bool,
    zero_divergence: bool,
    coverage_eligible: bool,
    cache_healthy: bool,
    targeted_within_budget: bool,
) -> bool:
    return (
        activation_gate
        and zero_divergence
        and coverage_eligible
        and cache_healthy
        and targeted_within_budget
    )


def activation_cache_error(evidence: ReplayEvidence) -> str | None:
    errors = sccache_error_count(evidence.sccache_after)
    if errors:
        return f"resource gate replay 出现 {errors} 个 sccache 缓存错误"
    return None


def replay_mismatches(results: list[ReplayResult]) -> list[str]:
    return [result.name for result in results if not result.matches]


def validate_activation_commands(manifest: ReplayManifest) -> None:
    if manifest.targeted_command != manifest.full_command:
        raise ReplayConfigurationError(
            "activation gate 的 targeted/full 必须运行同一资源门禁入口"
        )
    command = manifest.targeted_command
    if len(command) < 4 or tuple(command[:4]) != (
        "cargo",
        "xtask",
        "ci",
        "resource-gate",
    ):
        raise ReplayConfigurationError(
            "activation gate 只接受 cargo xtask ci resource-gate 作为比较入口"
        )
    remaining = command[4:]
    if remaining and (len(remaining) != 2 or remaining[0] != "--frontend-dir"):
        raise ReplayConfigurationError(
            "activation gate 资源门禁入口只允许可选的 --frontend-dir PATH"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--frontend-repository", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--activation-gate", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        manifest = load_manifest(args.manifest)
        if args.activation_gate:
            validate_activation_commands(manifest)
        run = run_replay(
            args.repository,
            args.frontend_repository,
            manifest,
            args.work_dir,
            manifest_fingerprint=sha256_file(args.manifest.resolve()),
            activation_gate=args.activation_gate,
        )
        write_report(args.report, run, activation_gate=args.activation_gate)
    except ReplayConfigurationError as error:
        print(error, file=sys.stderr)
        return 2
    results = run.results
    mismatches = replay_mismatches(results)
    if mismatches:
        print(
            "resource gate replay 存在分歧：" + ", ".join(mismatches), file=sys.stderr
        )
        return 1
    if args.activation_gate:
        cache_error = activation_cache_error(run.evidence)
        if cache_error is not None:
            print(cache_error, file=sys.stderr)
            return 1
    successful = [
        result.targeted.duration_ms
        for result in results
        if (
            result.expected == "pass"
            and result.targeted_mode == "targeted"
            and result.targeted.passed
            and result.targeted.decision.mode == "targeted"
        )
    ]
    print(
        f"resource gate replay 零分歧：{len(results)} 个案例，"
        f"targeted P95={percentile_nearest_rank(successful, 95)}ms"
    )
    targeted_p95 = percentile_nearest_rank(successful, 95)
    if args.activation_gate and (
        len(successful) < MINIMUM_SUCCESSFUL_CASES
        or targeted_p95 is None
        or targeted_p95 > TARGETED_P95_LIMIT_MS
    ):
        print(
            "resource gate replay targeted P95 超出激活预算，拒绝激活："
            f"{targeted_p95}ms > {TARGETED_P95_LIMIT_MS}ms",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

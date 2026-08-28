#!/usr/bin/env python3
"""在隔离 Git worktree 中比较 resource targeted 与 full gate 的通过/失败结果。"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


FORMAT_VERSION = 2
MINIMUM_CASES = 20
MINIMUM_SUCCESSFUL_CASES = 10
TARGETED_P95_LIMIT_MS = 60_000
TARGETED_ACTIVATION = "replay-verified-v1"
SHA_PATTERN = re.compile(r"[0-9a-fA-F]{40}")
REQUIRED_CATEGORIES = frozenset(
    {"addition", "field", "permission", "relation", "sql", "rename", "delete"}
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


@dataclass(frozen=True)
class ReplayResult:
    name: str
    category: str
    base: str
    head: str
    frontend_base: str
    frontend_head: str
    expected: str
    targeted: CommandResult
    full: CommandResult
    matches: bool


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
    return ReplayCase(
        name=raw["name"].strip(),
        category=raw["category"].strip(),
        base=raw["base"].lower(),
        head=raw["head"].lower(),
        frontend_base=raw["frontendBase"].lower(),
        frontend_head=raw["frontendHead"].lower(),
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


def run_replay(
    repository: Path,
    frontend_repository: Path,
    manifest: ReplayManifest,
    work_root: Path,
) -> list[ReplayResult]:
    repository = repository.resolve()
    frontend_repository = frontend_repository.resolve()
    if repository == frontend_repository:
        raise ReplayConfigurationError("前后端 replay 必须使用两个独立 Git 仓库")
    allowed = (repository / ".local-tests" / "resource-gate-replay").resolve()
    work_root = work_root.resolve()
    if not work_root.is_relative_to(allowed):
        raise ReplayConfigurationError(f"replay work-dir 必须位于 {allowed}")
    allowed.mkdir(parents=True, exist_ok=True)
    work_root.mkdir(parents=True, exist_ok=True)
    session = (work_root / f"r-{uuid.uuid4().hex[:12]}").resolve()
    if not session.is_relative_to(allowed):
        raise ReplayConfigurationError("replay 临时目录逃逸允许范围")
    session.mkdir()
    shared_target = (work_root / "target").resolve()
    if not shared_target.is_relative_to(allowed):
        raise ReplayConfigurationError("replay 共享 target 逃逸允许范围")
    results: list[ReplayResult] = []
    try:
        for case in manifest.cases:
            verify_commit_range(repository, case.base, case.head, case.name, "后端")
            verify_commit_range(
                frontend_repository,
                case.frontend_base,
                case.frontend_head,
                case.name,
                "前端",
                allow_unchanged=True,
            )
            targeted = execute_in_worktree(
                repository,
                frontend_repository,
                session / "t",
                case,
                manifest.targeted_command,
                shared_target,
                targeted=True,
            )
            full = execute_in_worktree(
                repository,
                frontend_repository,
                session / "f",
                case,
                manifest.full_command,
                shared_target,
                targeted=False,
            )
            results.append(
                ReplayResult(
                    name=case.name,
                    category=case.category,
                    base=case.base,
                    head=case.head,
                    frontend_base=case.frontend_base,
                    frontend_head=case.frontend_head,
                    expected=case.expected,
                    targeted=targeted,
                    full=full,
                    matches=(
                        targeted.passed == (case.expected == "pass")
                        and full.passed == (case.expected == "pass")
                    ),
                )
            )
    finally:
        subprocess.run(
            ["git", "worktree", "prune"],
            cwd=repository,
            check=False,
            capture_output=True,
        )
        subprocess.run(
            ["git", "worktree", "prune"],
            cwd=frontend_repository,
            check=False,
            capture_output=True,
        )
        if session.is_dir():
            try:
                session.rmdir()
            except OSError as error:
                raise ReplayConfigurationError(
                    f"replay 临时目录未完全回收：{session}：{error}"
                ) from error
    return results


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


def execute_in_worktree(
    repository: Path,
    frontend_repository: Path,
    worktree: Path,
    case: ReplayCase,
    command: tuple[str, ...],
    shared_target: Path,
    *,
    targeted: bool,
) -> CommandResult:
    backend_worktree = worktree / "b"
    frontend_worktree = worktree / "f"
    worktree.mkdir()
    add = subprocess.run(
        [
            "git",
            "worktree",
            "add",
            "--quiet",
            "--detach",
            str(backend_worktree),
            case.head,
        ],
        cwd=repository,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if add.returncode != 0:
        worktree.rmdir()
        raise ReplayConfigurationError(
            f"无法创建 replay worktree：{add.stderr.strip()}"
        )
    frontend_add = subprocess.run(
        [
            "git",
            "worktree",
            "add",
            "--quiet",
            "--detach",
            str(frontend_worktree),
            case.frontend_head,
        ],
        cwd=frontend_repository,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if frontend_add.returncode != 0:
        cleanup_worktree(repository, backend_worktree, "后端")
        worktree.rmdir()
        raise ReplayConfigurationError(
            f"无法创建前端 replay worktree：{frontend_add.stderr.strip()}"
        )
    try:
        environment = os.environ.copy()
        environment.update(
            {
                "RYFRAME_CI_BASE_SHA": case.base,
                "RYFRAME_CI_HEAD_SHA": case.head,
                "RYFRAME_CI_FRONTEND_REF": case.frontend_head,
                "RYFRAME_RESOURCE_GATE_REPLAY_CASE": case.name,
                "RYFRAME_DEVEX_TARGET_ROOT": str(shared_target),
            }
        )
        if targeted:
            environment["RYFRAME_RESOURCE_GATE_TARGETED"] = TARGETED_ACTIVATION
        else:
            environment.pop("RYFRAME_RESOURCE_GATE_TARGETED", None)
        started = time.perf_counter_ns()
        completed = subprocess.run(
            list(command_for_frontend(command, frontend_worktree)),
            cwd=backend_worktree,
            env=environment,
            check=False,
            capture_output=True,
        )
        duration_ms = (time.perf_counter_ns() - started) // 1_000_000
        return CommandResult(
            passed=completed.returncode == 0,
            return_code=completed.returncode,
            duration_ms=duration_ms,
        )
    finally:
        cleanup_worktree(frontend_repository, frontend_worktree, "前端")
        cleanup_worktree(repository, backend_worktree, "后端")
        try:
            worktree.rmdir()
        except OSError as error:
            raise ReplayConfigurationError(
                f"replay 临时案例目录未完全回收：{worktree}：{error}"
            ) from error


def cleanup_worktree(repository: Path, worktree: Path, label: str) -> None:
    cleanup = subprocess.run(
        ["git", "worktree", "remove", "--force", str(worktree)],
        cwd=repository,
        check=False,
        capture_output=True,
    )
    if cleanup.returncode != 0:
        raise ReplayConfigurationError(f"无法回收{label} replay worktree：{worktree}")


def write_report(
    path: Path,
    results: list[ReplayResult],
    *,
    activation_gate: bool = False,
) -> None:
    zero_divergence = bool(results) and all(result.matches for result in results)
    successful_targeted = [
        result.targeted.duration_ms
        for result in results
        if result.expected == "pass" and result.targeted.passed
    ]
    targeted_p95_ms = percentile_nearest_rank(successful_targeted, 95)
    targeted_within_budget = (
        len(successful_targeted) >= MINIMUM_SUCCESSFUL_CASES
        and targeted_p95_ms is not None
        and targeted_p95_ms <= TARGETED_P95_LIMIT_MS
    )
    document = {
        "formatVersion": FORMAT_VERSION,
        "caseCount": len(results),
        "successfulTargetedCaseCount": len(successful_targeted),
        "zeroDivergence": zero_divergence,
        "targetedP95Ms": targeted_p95_ms,
        "targetedP95LimitMs": TARGETED_P95_LIMIT_MS,
        "targetedWithinBudget": targeted_within_budget,
        "activationEligible": (
            activation_gate and zero_divergence and targeted_within_budget
        ),
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


def activation_performance_error(results: list[ReplayResult]) -> str | None:
    successful = [
        result.targeted.duration_ms
        for result in results
        if result.expected == "pass" and result.targeted.passed
    ]
    if len(successful) < MINIMUM_SUCCESSFUL_CASES:
        return (
            "resource gate replay 成功 targeted 案例不足："
            f"{len(successful)}/{MINIMUM_SUCCESSFUL_CASES}"
        )
    p95 = percentile_nearest_rank(successful, 95)
    if p95 is None or p95 > TARGETED_P95_LIMIT_MS:
        return f"resource gate targeted P95 超标：{p95}ms > {TARGETED_P95_LIMIT_MS}ms"
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
        results = run_replay(
            args.repository,
            args.frontend_repository,
            manifest,
            args.work_dir,
        )
        write_report(args.report, results, activation_gate=args.activation_gate)
    except ReplayConfigurationError as error:
        print(error, file=sys.stderr)
        return 2
    mismatches = replay_mismatches(results)
    if mismatches:
        print(
            "resource gate replay 存在分歧：" + ", ".join(mismatches), file=sys.stderr
        )
        return 1
    if args.activation_gate:
        performance_error = activation_performance_error(results)
        if performance_error is not None:
            print(performance_error, file=sys.stderr)
            return 1
    successful = [
        result.targeted.duration_ms
        for result in results
        if result.expected == "pass" and result.targeted.passed
    ]
    print(
        f"resource gate replay 零分歧：{len(results)} 个案例，"
        f"targeted P95={percentile_nearest_rank(successful, 95)}ms"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

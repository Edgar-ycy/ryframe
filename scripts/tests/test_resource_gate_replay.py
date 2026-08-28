from __future__ import annotations

import importlib.util
import json
import os
import shutil
import stat
import subprocess
import sys
import unittest
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "resource_gate_replay.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("resource_gate_replay", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

CATEGORIES = (
    "addition",
    "field",
    "permission",
    "relation",
    "sql",
    "rename",
    "delete",
)


@contextmanager
def isolated_test_dir(label: str) -> Iterator[Path]:
    root = (ROOT / ".local-tests").resolve()
    root.mkdir(exist_ok=True)
    path = (root / f"{label}-{uuid.uuid4().hex[:8]}").resolve()
    if not path.is_relative_to(root):
        raise AssertionError("测试目录逃逸 .local-tests")
    path.mkdir()
    try:
        yield path
    finally:
        if path.exists():
            shutil.rmtree(path, onerror=remove_readonly)


def remove_readonly(function: object, path: str, _: object) -> None:
    os.chmod(path, stat.S_IWRITE)
    function(path)  # type: ignore[operator]


class ResourceGateReplayTests(unittest.TestCase):
    def test_twenty_cases_execute_in_isolated_worktrees_with_zero_divergence(
        self,
    ) -> None:
        with isolated_test_dir("resource-replay") as fixture:
            repository = fixture / "repository"
            frontend_repository = fixture / "frontend-repository"
            repository.mkdir()
            frontend_repository.mkdir()
            self.initialize_repository(repository, frontend_repository)
            cases = self.create_cases(repository, frontend_repository)
            tools = fixture / "tools"
            tool_log = fixture / "tool.log"
            self.create_fake_tools(tools)
            manifest = fixture / "manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "formatVersion": 2,
                        "targetedCommand": [sys.executable, "check.py", "targeted"],
                        "fullCommand": [sys.executable, "check.py", "full"],
                        "cases": cases,
                    }
                ),
                encoding="utf-8",
            )
            report = fixture / "report.json"
            environment = os.environ.copy()
            environment["PATH"] = str(tools) + os.pathsep + environment.get("PATH", "")
            environment["RYFRAME_REPLAY_TEST_TOOL_LOG"] = str(tool_log)
            completed = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--repository",
                    str(repository),
                    "--frontend-repository",
                    str(frontend_repository),
                    "--manifest",
                    str(manifest),
                    "--work-dir",
                    str(repository / ".local-tests/resource-gate-replay/tests"),
                    "--report",
                    str(report),
                ],
                cwd=ROOT,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            document = json.loads(report.read_text(encoding="utf-8"))
            self.assertEqual(document["caseCount"], 20)
            self.assertTrue(document["zeroDivergence"])
            self.assertFalse(document["activationEligible"])
            self.assertEqual(document["successfulTargetedCaseCount"], 10)
            self.assertTrue(document["targetedWithinBudget"])
            self.assertLessEqual(document["targetedP95Ms"], 60_000)
            self.assertEqual(
                document["evidence"]["environment"]["CARGO_INCREMENTAL"], "0"
            )
            self.assertEqual(
                document["evidence"]["environment"]["RYFRAME_CI_RUST_GATE_PROFILE"],
                "standard",
            )
            self.assertTrue(
                document["evidence"]["manifest_fingerprint"].startswith("sha256:")
            )
            self.assertTrue(
                document["evidence"]["runner_fingerprint"].startswith("sha256:")
            )
            self.assertEqual(document["evidence"]["tools"]["sccache"], "sccache 0.test")
            self.assertTrue(
                document["evidence"]["tools_fingerprint"].startswith("sha256:")
            )
            self.assertEqual(
                {case["category"] for case in document["cases"]}, set(CATEGORIES)
            )
            self.assertTrue(all(case["matches"] for case in document["cases"]))
            self.assertTrue(
                all(
                    case["targeted"]["decision"]["recognized"]
                    for case in document["cases"]
                )
            )
            self.assertTrue(
                all(
                    not case["full"]["decision"]["recognized"]
                    and case["full"]["decision"]["mode"] == "full"
                    for case in document["cases"]
                )
            )
            self.assertEqual(document["evidence"]["prime"]["order"], 0)
            self.assertEqual(document["cases"][0]["targeted"]["order"], 1)
            self.assertEqual(document["cases"][0]["full"]["order"], 2)
            self.assertEqual(document["cases"][1]["full"]["order"], 3)
            self.assertEqual(document["cases"][1]["targeted"]["order"], 4)
            targets = {
                arm["target_fingerprint"]
                for case in document["cases"]
                for arm in (case["targeted"], case["full"])
            }
            self.assertEqual(len(targets), 40)
            installs = [
                line
                for line in tool_log.read_text(encoding="utf-8").splitlines()
                if line == "pnpm install --offline --frozen-lockfile"
            ]
            self.assertEqual(len(installs), 41)
            self.assertEqual(
                list(
                    (repository / ".local-tests/resource-gate-replay/tests").iterdir()
                ),
                [],
            )

    def test_manifest_and_mismatch_result_fail_closed(self) -> None:
        with isolated_test_dir("resource-replay-short") as fixture:
            path = fixture / "short.json"
            path.write_text(
                json.dumps(
                    {
                        "formatVersion": 2,
                        "targetedCommand": ["targeted"],
                        "fullCommand": ["full"],
                        "cases": [],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "至少需要 20"):
                MODULE.load_manifest(path)

        result = MODULE.ReplayResult(
            name="字段分歧",
            category="field",
            base="1" * 40,
            head="2" * 40,
            frontend_base="3" * 40,
            frontend_head="4" * 40,
            targeted_mode="targeted",
            expected="pass",
            targeted=self.command_result(True, 1, True, "targeted"),
            full=self.command_result(False, 1, False, "full"),
            matches=False,
        )
        self.assertEqual(MODULE.replay_mismatches([result]), ["字段分歧"])

        valid_cases = tuple(
            MODULE.ReplayCase(
                f"case-{index}",
                CATEGORIES[index % 7],
                f"{index + 1:040x}",
                f"{index + 101:040x}",
                f"{index + 201:040x}",
                f"{index + 301:040x}",
                "targeted" if index % 2 == 0 else "full",
                "pass" if index % 2 == 0 else "fail",
            )
            for index in range(20)
        )
        valid = MODULE.ReplayManifest(
            ("cargo", "xtask", "ci", "resource-gate"),
            ("cargo", "xtask", "ci", "resource-gate"),
            valid_cases,
        )
        MODULE.validate_case_coverage(valid_cases)
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "targeted mode"):
            MODULE.require_arm(
                valid_cases[0],
                self.command_result(True, 1, True, "full"),
                targeted=True,
            )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "full arm"):
            MODULE.require_arm(
                valid_cases[0],
                self.command_result(True, 1, True, "full"),
                targeted=False,
            )
        duplicate_range = (
            *valid_cases[:-1],
            MODULE.ReplayCase(
                "duplicate-range",
                valid_cases[-1].category,
                valid_cases[0].base,
                valid_cases[0].head,
                valid_cases[-1].frontend_base,
                valid_cases[-1].frontend_head,
                valid_cases[-1].targeted_mode,
                valid_cases[-1].expected,
            ),
        )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "范围必须唯一"):
            MODULE.validate_case_coverage(duplicate_range)
        all_pass = tuple(
            MODULE.ReplayCase(
                case.name,
                case.category,
                case.base,
                case.head,
                case.frontend_base,
                case.frontend_head,
                case.targeted_mode,
                "pass",
            )
            for case in valid_cases
        )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "同时覆盖"):
            MODULE.validate_case_coverage(all_pass)
        MODULE.validate_activation_commands(valid)
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "同一资源门禁"):
            MODULE.validate_activation_commands(
                MODULE.ReplayManifest(
                    valid.targeted_command, ("cargo", "test"), valid_cases
                )
            )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "只允许可选"):
            MODULE.validate_activation_commands(
                MODULE.ReplayManifest(
                    (*valid.targeted_command, "--help"),
                    (*valid.full_command, "--help"),
                    valid_cases,
                )
            )

    def test_activation_requires_ten_successes_and_p95_at_most_sixty_seconds(
        self,
    ) -> None:
        def result(index: int, duration_ms: int, passed: bool = True):
            expected = "pass" if passed else "fail"
            return MODULE.ReplayResult(
                name=f"case-{index}",
                category=CATEGORIES[index % len(CATEGORIES)],
                base=f"{index + 1:040x}",
                head=f"{index + 101:040x}",
                frontend_base=f"{index + 201:040x}",
                frontend_head=f"{index + 301:040x}",
                targeted_mode="targeted",
                expected=expected,
                targeted=self.command_result(passed, duration_ms, True, "targeted"),
                full=self.command_result(passed, duration_ms, False, "full"),
                matches=True,
            )

        too_few = [result(index, 1_000) for index in range(9)]
        self.assertIn("不足", MODULE.activation_performance_error(too_few))
        within_budget = [result(index, 10_000 + index) for index in range(10)]
        self.assertIsNone(MODULE.activation_performance_error(within_budget))
        over_budget = [*within_budget[:-1], result(10, 60_001)]
        self.assertIn("P95", MODULE.activation_performance_error(over_budget))
        self.assertEqual(MODULE.percentile_nearest_rank([3, 1, 2], 95), 3)

    def test_frontend_argument_is_stable_across_temporary_worktrees(self) -> None:
        command = (
            "cargo",
            "xtask",
            "ci",
            "resource-gate",
            "--frontend-dir",
            "../frontend",
        )
        frontend = (ROOT / "../frontend").resolve()
        normalized = MODULE.command_for_frontend(command, frontend)
        self.assertEqual(Path(normalized[-1]), frontend)
        appended = MODULE.command_for_frontend(("gate",), frontend)
        self.assertEqual(appended, ("gate", "--frontend-dir", str(frontend)))
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "缺少路径"):
            MODULE.command_for_frontend(("gate", "--frontend-dir"), frontend)

    def test_decision_environment_order_and_cleanup_fail_closed(self) -> None:
        with isolated_test_dir("resource-replay-decision") as fixture:
            decision_path = fixture / "decision.json"
            decision_path.write_text(
                json.dumps(
                    {
                        "formatVersion": 1,
                        "recognized": True,
                        "mode": "targeted",
                        "fallback": None,
                        "steps": ["resource-drift"],
                    }
                ),
                encoding="utf-8",
            )
            decision = MODULE.load_decision(decision_path)
            self.assertTrue(decision.recognized)
            self.assertEqual(decision.mode, "targeted")
            decision_path.write_text(
                json.dumps(
                    {
                        "formatVersion": 1,
                        "recognized": True,
                        "mode": "full",
                        "fallback": None,
                        "steps": ["full-rust-gate"],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "fallback"):
                MODULE.load_decision(decision_path)

            cache = fixture / "cache"
            with mock.patch.dict(
                os.environ,
                {
                    "RYFRAME_CI_RUST_GATE_PROFILE": "windows-smoke",
                    "RYFRAME_CI_BACKEND_HEAD": "stale",
                    "RUSTFLAGS": "-Ctarget-cpu=native",
                    "CARGO_INCREMENTAL": "1",
                    "SCCACHE_GHA_ENABLED": "true",
                },
                clear=False,
            ):
                environment = MODULE.controlled_environment(
                    "D:/tools/sccache", cache, False
                )
            self.assertEqual(environment["RYFRAME_CI_RUST_GATE_PROFILE"], "standard")
            self.assertEqual(environment["CARGO_INCREMENTAL"], "0")
            self.assertEqual(environment["RYFRAME_VERIFY_JOBS"], "8")
            self.assertNotIn("RYFRAME_CI_BACKEND_HEAD", environment)
            self.assertNotIn("RUSTFLAGS", environment)
            self.assertNotIn("SCCACHE_GHA_ENABLED", environment)
            with mock.patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(
                    MODULE.ReplayConfigurationError, "RYFRAME_MYSQL_INTEGRATION"
                ):
                    MODULE.controlled_environment("sccache", cache, True)

        self.assertEqual(MODULE.arm_order(1), (True, False))
        self.assertEqual(MODULE.arm_order(2), (False, True))
        entries = (
            (True, Path("backend"), Path("frontend-worktree"), "前端"),
            (True, Path("backend"), Path("backend-worktree"), "后端"),
        )
        with mock.patch.object(
            MODULE, "cleanup_worktree", side_effect=["前端清理失败", None]
        ) as cleanup:
            errors = MODULE.cleanup_created_worktrees(entries)
        self.assertEqual(errors, ["前端清理失败"])
        self.assertEqual(cleanup.call_count, 2)
        with self.assertRaisesRegex(
            MODULE.ReplayConfigurationError, "原始门禁失败.*前端清理失败"
        ):
            MODULE.fail_on_cleanup(
                MODULE.ReplayConfigurationError("原始门禁失败"),
                "案例清理",
                errors,
            )
        failed = subprocess.CompletedProcess(
            ["git", "worktree", "remove"], 1, "", "locked"
        )
        succeeded = subprocess.CompletedProcess(
            ["git", "worktree", "remove"], 0, "", ""
        )
        with (
            mock.patch.object(MODULE.os, "name", "nt"),
            mock.patch.object(
                MODULE.subprocess, "run", side_effect=[failed, succeeded]
            ) as run,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            self.assertIsNone(
                MODULE.cleanup_worktree(Path("repository"), Path("worktree"), "Windows")
            )
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(0.1)

    def test_repository_pair_rejects_shared_git_common_dir(self) -> None:
        with isolated_test_dir("resource-replay-common-dir") as fixture:
            repository = fixture / "repository"
            linked = fixture / "linked"
            repository.mkdir()
            self.git(repository, "init", "--quiet")
            self.git(repository, "config", "user.email", "ci@example.invalid")
            self.git(repository, "config", "user.name", "CI")
            (repository / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            self.git(repository, "add", "Cargo.toml")
            self.git(repository, "commit", "--quiet", "-m", "base")
            self.git(repository, "worktree", "add", "--detach", str(linked), "HEAD")
            try:
                primary = MODULE.repository_identity(repository, "主", "Cargo.toml")
                secondary = MODULE.repository_identity(linked, "副", "Cargo.toml")
                with self.assertRaisesRegex(
                    MODULE.ReplayConfigurationError, "common-dir"
                ):
                    MODULE.validate_repository_pair(primary, secondary)
            finally:
                self.git(repository, "worktree", "remove", "--force", str(linked))

    def initialize_repository(
        self, repository: Path, frontend_repository: Path
    ) -> None:
        self.git(repository, "init", "--quiet")
        self.git(repository, "config", "user.email", "ci@example.invalid")
        self.git(repository, "config", "user.name", "CI")
        (repository / "check.py").write_text(
            """from __future__ import annotations
import os
import json
from pathlib import Path
import sys

mode = sys.argv[1]
activation = os.environ.get("RYFRAME_RESOURCE_GATE_TARGETED")
if mode == "targeted" and activation != "replay-verified-v1":
    raise SystemExit(2)
if mode == "full" and activation is not None:
    raise SystemExit(3)
if os.environ.get("RYFRAME_CI_RUST_GATE_PROFILE") != "standard":
    raise SystemExit(4)
if os.environ.get("CARGO_INCREMENTAL") != "0":
    raise SystemExit(5)
if os.environ.get("RYFRAME_VERIFY_JOBS") != "8" or os.environ.get("RYFRAME_CI_TEST_JOBS") != "4":
    raise SystemExit(6)
if "RUSTFLAGS" in os.environ or "CARGO_ENCODED_RUSTFLAGS" in os.environ:
    raise SystemExit(7)
option = sys.argv.index("--frontend-dir")
frontend = Path(sys.argv[option + 1])
if not frontend.is_absolute():
    raise SystemExit(8)
if not frontend.joinpath("node_modules").is_dir():
    raise SystemExit(9)
if frontend.joinpath("scenario.txt").read_text().strip() != Path("scenario.txt").read_text().strip():
    raise SystemExit(10)
target = Path(os.environ.get("RYFRAME_DEVEX_TARGET_ROOT", ""))
if not target.is_absolute() or not target.is_relative_to(Path.cwd()):
    raise SystemExit(11)
actual_mode = Path("targeted-mode.txt").read_text().strip() if mode == "targeted" else "full"
decision = {
    "formatVersion": 1,
    "recognized": mode == "targeted",
    "mode": actual_mode,
    "fallback": None if actual_mode == "targeted" else "fixture full fallback",
    "steps": ["resource-drift", "resource-workspace"] if actual_mode == "targeted" else ["resource-drift", "full-rust-gate"],
}
decision_path = Path(os.environ["RYFRAME_RESOURCE_GATE_DECISION_FILE"])
decision_path.write_text(json.dumps(decision), encoding="utf-8")
raise SystemExit(0 if Path("status.txt").read_text().strip() == "pass" else 1)
""",
            encoding="utf-8",
        )
        (repository / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\n', encoding="utf-8"
        )
        (repository / "status.txt").write_text("pass\n", encoding="utf-8")
        (repository / "scenario.txt").write_text("base\n", encoding="utf-8")
        (repository / "targeted-mode.txt").write_text("targeted\n", encoding="utf-8")
        self.git(repository, "add", ".")
        self.git(repository, "commit", "--quiet", "-m", "fixture base")

        self.git(frontend_repository, "init", "--quiet")
        self.git(frontend_repository, "config", "user.email", "ci@example.invalid")
        self.git(frontend_repository, "config", "user.name", "CI")
        (frontend_repository / ".gitignore").write_text(
            "node_modules/\n", encoding="utf-8"
        )
        (frontend_repository / "package.json").write_text(
            json.dumps(
                {
                    "name": "resource-replay-fixture",
                    "private": True,
                    "packageManager": "pnpm@11.20.0",
                }
            ),
            encoding="utf-8",
        )
        (frontend_repository / "pnpm-lock.yaml").write_text(
            "lockfileVersion: '9.0'\n", encoding="utf-8"
        )
        (frontend_repository / "scenario.txt").write_text("base\n", encoding="utf-8")
        self.git(frontend_repository, "add", ".")
        self.git(frontend_repository, "commit", "--quiet", "-m", "fixture base")

    def create_cases(
        self, repository: Path, frontend_repository: Path
    ) -> list[dict[str, str]]:
        cases: list[dict[str, str]] = []
        for index in range(20):
            base = self.git_output(repository, "rev-parse", "HEAD")
            frontend_base = self.git_output(frontend_repository, "rev-parse", "HEAD")
            category = CATEGORIES[index % len(CATEGORIES)]
            status = "pass" if index % 2 == 0 else "fail"
            targeted_mode = "targeted" if index % 2 == 0 else "full"
            case_name = f"{category}-{index}"
            (repository / "status.txt").write_text(status + "\n", encoding="utf-8")
            (repository / "scenario.txt").write_text(
                f"{category}-{index}\n", encoding="utf-8"
            )
            (repository / "targeted-mode.txt").write_text(
                targeted_mode + "\n", encoding="utf-8"
            )
            self.git(
                repository,
                "add",
                "status.txt",
                "scenario.txt",
                "targeted-mode.txt",
            )
            self.git(
                repository,
                "commit",
                "--quiet",
                "-m",
                f"{category} case {index}",
            )
            (frontend_repository / "scenario.txt").write_text(
                case_name + "\n", encoding="utf-8"
            )
            self.git(frontend_repository, "add", "scenario.txt")
            self.git(
                frontend_repository,
                "commit",
                "--quiet",
                "-m",
                f"{category} frontend case {index}",
            )
            cases.append(
                {
                    "name": case_name,
                    "category": category,
                    "base": base,
                    "head": self.git_output(repository, "rev-parse", "HEAD"),
                    "frontendBase": frontend_base,
                    "frontendHead": self.git_output(
                        frontend_repository, "rev-parse", "HEAD"
                    ),
                    "targetedMode": targeted_mode,
                    "expected": status,
                }
            )
        return cases

    def command_result(
        self,
        passed: bool,
        duration_ms: int,
        recognized: bool,
        mode: str,
    ):
        decision = MODULE.ResourceGateDecision(
            recognized,
            mode,
            None if mode == "targeted" else "fixture fallback",
            ("resource-drift",),
        )
        return MODULE.CommandResult(
            passed,
            0 if passed else 1,
            duration_ms,
            0,
            decision,
            f"sha256:{duration_ms:064x}",
        )

    def create_fake_tools(self, directory: Path) -> None:
        directory.mkdir()
        self.create_fake_command(
            directory,
            "corepack",
            """import os
from pathlib import Path
import sys

arguments = " ".join(sys.argv[1:])
log = os.environ.get("RYFRAME_REPLAY_TEST_TOOL_LOG")
if log:
    with Path(log).open("a", encoding="utf-8") as output:
        output.write(arguments + "\\n")
if sys.argv[1:] == ["--version"]:
    print("corepack 0.test")
elif sys.argv[1:] == ["pnpm", "--version"]:
    print("11.20.0-test")
elif sys.argv[1:] == ["pnpm", "install", "--offline", "--frozen-lockfile"]:
    Path("node_modules").mkdir(exist_ok=True)
else:
    raise SystemExit(2)
""",
        )
        self.create_fake_command(
            directory,
            "sccache",
            """import json
import sys

if sys.argv[1:] == ["--version"]:
    print("sccache 0.test")
elif sys.argv[1:2] == ["--show-stats"]:
    print(json.dumps({"compile_requests": 0, "cache_hits": {"counts": {}}}))
else:
    raise SystemExit(0)
""",
        )

    def create_fake_command(self, directory: Path, name: str, source: str) -> None:
        if os.name == "nt":
            script = directory / f"{name}.py"
            script.write_text(source, encoding="utf-8")
            command = directory / f"{name}.cmd"
            command.write_text(
                f'@echo off\r\n"{sys.executable}" "%~dp0{name}.py" %*\r\n',
                encoding="utf-8",
            )
            return
        command = directory / name
        command.write_text("#!/usr/bin/env python3\n" + source, encoding="utf-8")
        command.chmod(command.stat().st_mode | stat.S_IXUSR)

    def git(self, repository: Path, *arguments: str) -> None:
        subprocess.run(
            ["git", *arguments],
            cwd=repository,
            check=True,
            capture_output=True,
        )

    def git_output(self, repository: Path, *arguments: str) -> str:
        return subprocess.check_output(
            ["git", *arguments], cwd=repository, text=True, encoding="utf-8"
        ).strip()


if __name__ == "__main__":
    unittest.main()

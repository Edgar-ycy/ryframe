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
                {case["category"] for case in document["cases"]}, set(CATEGORIES)
            )
            self.assertTrue(all(case["matches"] for case in document["cases"]))
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
            expected="pass",
            targeted=MODULE.CommandResult(True, 0, 1),
            full=MODULE.CommandResult(False, 1, 1),
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
        duplicate_range = (
            *valid_cases[:-1],
            MODULE.ReplayCase(
                "duplicate-range",
                valid_cases[-1].category,
                valid_cases[0].base,
                valid_cases[0].head,
                valid_cases[-1].frontend_base,
                valid_cases[-1].frontend_head,
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
            command = MODULE.CommandResult(passed, 0 if passed else 1, duration_ms)
            return MODULE.ReplayResult(
                name=f"case-{index}",
                category=CATEGORIES[index % len(CATEGORIES)],
                base=f"{index + 1:040x}",
                head=f"{index + 101:040x}",
                frontend_base=f"{index + 201:040x}",
                frontend_head=f"{index + 301:040x}",
                expected=expected,
                targeted=command,
                full=command,
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

    def initialize_repository(
        self, repository: Path, frontend_repository: Path
    ) -> None:
        self.git(repository, "init", "--quiet")
        self.git(repository, "config", "user.email", "ci@example.invalid")
        self.git(repository, "config", "user.name", "CI")
        (repository / "check.py").write_text(
            """from __future__ import annotations
import os
from pathlib import Path
import sys

mode = sys.argv[1]
activation = os.environ.get("RYFRAME_RESOURCE_GATE_TARGETED")
if mode == "targeted" and activation != "replay-verified-v1":
    raise SystemExit(2)
if mode == "full" and activation is not None:
    raise SystemExit(3)
if not os.environ.get("RYFRAME_RESOURCE_GATE_REPLAY_CASE"):
    raise SystemExit(4)
option = sys.argv.index("--frontend-dir")
frontend = Path(sys.argv[option + 1])
if not frontend.is_absolute():
    raise SystemExit(5)
if frontend.joinpath("scenario.txt").read_text().strip() != os.environ["RYFRAME_RESOURCE_GATE_REPLAY_CASE"]:
    raise SystemExit(6)
shared_target = Path(os.environ.get("RYFRAME_DEVEX_TARGET_ROOT", ""))
if not shared_target.is_absolute() or "resource-gate-replay" not in shared_target.parts:
    raise SystemExit(7)
raise SystemExit(0 if Path("status.txt").read_text().strip() == "pass" else 1)
""",
            encoding="utf-8",
        )
        (repository / "status.txt").write_text("pass\n", encoding="utf-8")
        (repository / "scenario.txt").write_text("base\n", encoding="utf-8")
        self.git(repository, "add", ".")
        self.git(repository, "commit", "--quiet", "-m", "fixture base")

        self.git(frontend_repository, "init", "--quiet")
        self.git(frontend_repository, "config", "user.email", "ci@example.invalid")
        self.git(frontend_repository, "config", "user.name", "CI")
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
            case_name = f"{category}-{index}"
            (repository / "status.txt").write_text(status + "\n", encoding="utf-8")
            (repository / "scenario.txt").write_text(
                f"{category}-{index}\n", encoding="utf-8"
            )
            self.git(repository, "add", "status.txt", "scenario.txt")
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
                    "expected": status,
                }
            )
        return cases

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

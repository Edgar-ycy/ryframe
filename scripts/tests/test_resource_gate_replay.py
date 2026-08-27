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
    path = (root / f"{label}-{uuid.uuid4().hex}").resolve()
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
    def test_twenty_cases_execute_in_isolated_worktrees_with_zero_divergence(self) -> None:
        with isolated_test_dir("resource-replay") as fixture:
            repository = fixture / "repository"
            repository.mkdir()
            self.initialize_repository(repository)
            cases = self.create_cases(repository)
            manifest = fixture / "manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "formatVersion": 1,
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
            self.assertEqual(
                {case["category"] for case in document["cases"]}, set(CATEGORIES)
            )
            self.assertTrue(all(case["matches"] for case in document["cases"]))
            self.assertEqual(
                list((repository / ".local-tests/resource-gate-replay/tests").iterdir()),
                [],
            )

    def test_manifest_and_mismatch_result_fail_closed(self) -> None:
        with isolated_test_dir("resource-replay-short") as fixture:
            path = fixture / "short.json"
            path.write_text(
                json.dumps(
                    {
                        "formatVersion": 1,
                        "targetedCommand": ["targeted"],
                        "fullCommand": ["full"],
                        "cases": [],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                MODULE.ReplayConfigurationError, "至少需要 20"
            ):
                MODULE.load_manifest(path)

        result = MODULE.ReplayResult(
            name="字段分歧",
            category="field",
            base="1" * 40,
            head="2" * 40,
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
                valid_cases[-1].expected,
            ),
        )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "范围必须唯一"):
            MODULE.validate_case_coverage(duplicate_range)
        all_pass = tuple(
            MODULE.ReplayCase(
                case.name, case.category, case.base, case.head, "pass"
            )
            for case in valid_cases
        )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "同时覆盖"):
            MODULE.validate_case_coverage(all_pass)
        MODULE.validate_activation_commands(valid)
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "同一资源门禁"):
            MODULE.validate_activation_commands(
                MODULE.ReplayManifest(valid.targeted_command, ("cargo", "test"), valid_cases)
            )
        with self.assertRaisesRegex(MODULE.ReplayConfigurationError, "只允许可选"):
            MODULE.validate_activation_commands(
                MODULE.ReplayManifest(
                    (*valid.targeted_command, "--help"),
                    (*valid.full_command, "--help"),
                    valid_cases,
                )
            )

    def initialize_repository(self, repository: Path) -> None:
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
raise SystemExit(0 if Path("status.txt").read_text().strip() == "pass" else 1)
""",
            encoding="utf-8",
        )
        (repository / "status.txt").write_text("pass\n", encoding="utf-8")
        (repository / "scenario.txt").write_text("base\n", encoding="utf-8")
        self.git(repository, "add", ".")
        self.git(repository, "commit", "--quiet", "-m", "fixture base")

    def create_cases(self, repository: Path) -> list[dict[str, str]]:
        cases: list[dict[str, str]] = []
        for index in range(20):
            base = self.git_output(repository, "rev-parse", "HEAD")
            category = CATEGORIES[index % len(CATEGORIES)]
            status = "pass" if index % 2 == 0 else "fail"
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
            cases.append(
                {
                    "name": f"{category}-{index}",
                    "category": category,
                    "base": base,
                    "head": self.git_output(repository, "rev-parse", "HEAD"),
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

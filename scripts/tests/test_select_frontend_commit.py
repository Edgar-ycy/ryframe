from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path
from collections.abc import Iterator
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "select_frontend_commit.py"
SPEC = importlib.util.spec_from_file_location("select_frontend_commit", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TEMP_ROOT = SCRIPT.parents[1] / ".local-tests/python-unit"
TEMP_ROOT.mkdir(parents=True, exist_ok=True)


@contextmanager
def test_directory() -> Iterator[Path]:
    path = TEMP_ROOT / f"select-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path)


class SelectFrontendCommitTests(unittest.TestCase):
    def test_uses_main_when_contract_is_unchanged(self) -> None:
        self.assertEqual(
            MODULE.select_frontend_ref("Frontend-Commit: invalid", False),
            "main",
        )

    def test_accepts_one_exact_full_sha(self) -> None:
        sha = "A" * 40
        self.assertEqual(
            MODULE.select_frontend_ref(f"说明\nFrontend-Commit: {sha}\n", True),
            sha.lower(),
        )

    def test_accepts_windows_and_legacy_line_endings(self) -> None:
        sha = "C" * 40
        for body in (
            f"说明\r\nFrontend-Commit: {sha}\r\n",
            f"说明\rFrontend-Commit: {sha}\r",
            f"说明\nFrontend-Commit: {sha}",
        ):
            with self.subTest(body=repr(body)):
                self.assertEqual(
                    MODULE.select_frontend_ref(body, True),
                    sha.lower(),
                )

    def test_windows_prefers_valid_marker_when_contract_is_unchanged(self) -> None:
        sha = "B" * 40
        self.assertEqual(
            MODULE.select_frontend_ref(
                f"说明\nFrontend-Commit: {sha}\n",
                False,
                prefer_marker=True,
            ),
            sha.lower(),
        )
        self.assertEqual(
            MODULE.select_frontend_ref("没有配套提交", False, prefer_marker=True),
            "main",
        )

    def test_windows_rejects_invalid_marker_instead_of_silently_using_main(self) -> None:
        for body in ("Frontend-Commit: abc123", "Frontend-Commit: " + "1" * 41):
            with self.subTest(body=body), self.assertRaises(ValueError):
                MODULE.select_frontend_ref(body, False, prefer_marker=True)

    def test_rejects_missing_or_short_sha(self) -> None:
        for body in ("", "Frontend-Commit: abc123"):
            with self.subTest(body=body), self.assertRaises(ValueError):
                MODULE.select_frontend_ref(body, True)

    def test_rejects_duplicate_or_embedded_marker(self) -> None:
        sha = "1" * 40
        bodies = (
            f"Frontend-Commit: {sha}\nFrontend-Commit: {sha}",
            f"- Frontend-Commit: {sha}",
            f"Frontend-Commit: {sha} trailing",
        )
        for body in bodies:
            with self.subTest(body=body), self.assertRaises(ValueError):
                MODULE.select_frontend_ref(body, True)

    def test_git_classification_uses_only_the_openapi_snapshot(self) -> None:
        sha = "1" * 40
        with mock.patch.object(MODULE.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 1, b"", b"")
            changed = MODULE.contract_changed_from_git(Path("backend"), sha)

        self.assertTrue(changed)
        run.assert_called_once_with(
            [
                "git",
                "diff",
                "--quiet",
                sha,
                "HEAD",
                "--",
                "openapi/openapi.json",
            ],
            cwd=Path("backend"),
            check=False,
            capture_output=True,
        )

    def test_candidate_generation_uses_fixed_target_and_validates_snapshot(self) -> None:
        sha = "2" * 40
        with test_directory() as root:
            (root / "openapi").mkdir()
            committed = b'{"openapi":"3.1.0"}\n'
            (root / "openapi/openapi.json").write_bytes(committed)
            candidate = root / "artifacts/candidate.json"

            def run(arguments: list[str], *, cwd: Path, capture_output: bool = False):
                self.assertEqual(cwd, root)
                if arguments[0] == "cargo":
                    candidate.parent.mkdir(parents=True, exist_ok=True)
                    candidate.write_bytes(committed)
                    return subprocess.CompletedProcess(arguments, 0, b"", b"")
                self.assertEqual(
                    arguments,
                    ["git", "show", f"{sha}:openapi/openapi.json"],
                )
                self.assertTrue(capture_output)
                return subprocess.CompletedProcess(arguments, 0, b"old", b"")

            with mock.patch.object(MODULE, "_run", side_effect=run) as invoked:
                self.assertTrue(
                    MODULE.generate_and_classify_candidate(root, sha, candidate)
                )

            cargo_arguments = invoked.call_args_list[0].args[0]
            self.assertEqual(
                cargo_arguments,
                [
                    "cargo",
                    "run",
                    "--locked",
                    "--target-dir",
                    "target/ci/backend",
                    "-p",
                    "ryframe-api",
                    "--bin",
                    "export_openapi",
                    "--",
                    str(candidate),
                ],
            )

    def test_ci_selection_handles_pr_and_release_without_yaml_decisions(self) -> None:
        sha = "3" * 40
        with test_directory() as directory:
            event = directory / "event.json"
            event.write_text(
                json.dumps({"pull_request": {"body": f"Frontend-Commit: {sha}"}}),
                encoding="utf-8",
            )
            with mock.patch.object(
                MODULE, "commit_exists_in_worktree", return_value=True
            ), mock.patch.object(
                MODULE, "contract_changed_from_git", return_value=True
            ) as classify:
                ref, changed = MODULE.select_ci_frontend_ref(
                    event_name="pull_request",
                    event_path=event,
                    backend_worktree=Path("backend"),
                    base_sha="4" * 40,
                    prefer_marker=True,
                    candidate_path=None,
                    release_ref=None,
                )
            self.assertEqual(ref, sha)
            self.assertTrue(changed)
            classify.assert_called_once_with(Path("backend"), "4" * 40)

        self.assertEqual(
            MODULE.select_ci_frontend_ref(
                event_name="push",
                event_path=None,
                backend_worktree=Path("backend"),
                base_sha=None,
                prefer_marker=False,
                candidate_path=None,
                release_ref="refs/tags/v1.2.3",
            ),
            ("v1.2.3", False),
        )

    def test_resource_selection_uses_main_only_when_pr_base_is_unresolvable(self) -> None:
        with test_directory() as directory:
            event = directory / "event.json"
            event.write_text(
                json.dumps({"pull_request": {"body": ""}}), encoding="utf-8"
            )
            with mock.patch.object(
                MODULE, "commit_exists_in_worktree", return_value=False
            ) as exists, mock.patch.object(MODULE, "contract_changed_from_git") as classify:
                selected = MODULE.select_ci_frontend_ref(
                    event_name="pull_request",
                    event_path=event,
                    backend_worktree=Path("backend"),
                    base_sha="4" * 40,
                    prefer_marker=True,
                    candidate_path=None,
                    release_ref=None,
                    fallback_main_on_invalid_base=True,
                )

        self.assertEqual(selected, ("main", False))
        exists.assert_called_once_with(Path("backend"), "4" * 40)
        classify.assert_not_called()

    def test_resource_selection_preserves_explicit_frontend_marker_without_base(self) -> None:
        with test_directory() as directory:
            event = directory / "event.json"
            frontend_sha = "5" * 40
            event.write_text(
                json.dumps(
                    {
                        "pull_request": {
                            "body": f"Frontend-Commit: {frontend_sha}"
                        }
                    }
                ),
                encoding="utf-8",
            )
            selected = MODULE.select_ci_frontend_ref(
                event_name="pull_request",
                event_path=event,
                backend_worktree=Path("backend"),
                base_sha=None,
                prefer_marker=True,
                candidate_path=None,
                release_ref=None,
                fallback_main_on_invalid_base=True,
            )

        self.assertEqual(selected, (frontend_sha, False))

    def test_unresolvable_base_fails_before_candidate_generation(self) -> None:
        with test_directory() as directory:
            event = directory / "event.json"
            event.write_text(
                json.dumps({"pull_request": {"body": ""}}), encoding="utf-8"
            )
            with mock.patch.object(
                MODULE, "commit_exists_in_worktree", return_value=False
            ), mock.patch.object(MODULE, "generate_and_classify_candidate") as generate:
                with self.assertRaisesRegex(ValueError, "基线提交无法解析"):
                    MODULE.select_ci_frontend_ref(
                        event_name="pull_request",
                        event_path=event,
                        backend_worktree=Path("backend"),
                        base_sha="6" * 40,
                        prefer_marker=False,
                        candidate_path=directory / "candidate.json",
                        release_ref=None,
                    )
            generate.assert_not_called()

    def test_invalid_event_fails_before_candidate_generation(self) -> None:
        with test_directory() as directory:
            event = directory / "event.json"
            event.write_text("not-json", encoding="utf-8")
            with mock.patch.object(
                MODULE, "generate_and_classify_candidate"
            ) as generate:
                with self.assertRaisesRegex(ValueError, "无法读取 CI 事件"):
                    MODULE.select_ci_frontend_ref(
                        event_name="pull_request",
                        event_path=event,
                        backend_worktree=Path("backend"),
                        base_sha="7" * 40,
                        prefer_marker=False,
                        candidate_path=directory / "candidate.json",
                        release_ref=None,
                    )
            generate.assert_not_called()

    def test_private_entry_rejects_public_arguments(self) -> None:
        with mock.patch.object(
            MODULE.sys, "argv", [str(SCRIPT), "--event-name", "push"]
        ):
            with self.assertRaisesRegex(ValueError, "不接受命令行参数"):
                MODULE._private_request()

    def test_private_request_requires_exact_json_fields(self) -> None:
        valid = {
            "event_name": "push",
            "event": None,
            "backend_worktree": str(SCRIPT.parents[1]),
            "base_sha": None,
            "prefer_marker": False,
            "candidate_openapi": None,
            "release_ref": "refs/heads/main",
            "fallback_main_on_invalid_base": False,
        }
        with mock.patch.object(MODULE.sys, "argv", [str(SCRIPT)]), mock.patch.dict(
            MODULE.os.environ,
            {MODULE.REQUEST_ENV: json.dumps(valid)},
            clear=False,
        ):
            self.assertEqual(MODULE._private_request(), valid)
        valid["unknown"] = True
        with mock.patch.object(MODULE.sys, "argv", [str(SCRIPT)]), mock.patch.dict(
            MODULE.os.environ,
            {MODULE.REQUEST_ENV: json.dumps(valid)},
            clear=False,
        ):
            with self.assertRaisesRegex(ValueError, "字段"):
                MODULE._private_request()

    def test_private_main_preserves_github_output_contract(self) -> None:
        with test_directory() as directory:
            output = directory / "github-output.txt"
            request = {
                "event_name": "push",
                "event": None,
                "backend_worktree": str(SCRIPT.parents[1]),
                "base_sha": None,
                "prefer_marker": False,
                "candidate_openapi": None,
                "release_ref": "refs/heads/main",
                "fallback_main_on_invalid_base": False,
            }
            with mock.patch.object(MODULE.sys, "argv", [str(SCRIPT)]), mock.patch.dict(
                MODULE.os.environ,
                {
                    MODULE.REQUEST_ENV: json.dumps(request),
                    "GITHUB_OUTPUT": str(output),
                },
                clear=False,
            ), mock.patch("builtins.print"):
                MODULE.main()
            self.assertEqual(output.read_text(encoding="utf-8"), "ref=main\nchanged=false\n")


if __name__ == "__main__":
    unittest.main()

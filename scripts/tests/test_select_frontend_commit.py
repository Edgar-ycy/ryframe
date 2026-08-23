from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "select_frontend_commit.py"
SPEC = importlib.util.spec_from_file_location("select_frontend_commit", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


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


if __name__ == "__main__":
    unittest.main()

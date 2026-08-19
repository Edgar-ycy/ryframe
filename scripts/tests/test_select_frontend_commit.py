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

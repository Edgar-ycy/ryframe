from __future__ import annotations

import importlib.util
import os
import shutil
import stat
import subprocess
import sys
import unittest
import uuid
from contextlib import contextmanager
from collections.abc import Iterator
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "check_python_environment.py"
SPEC = importlib.util.spec_from_file_location("check_python_environment", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


@contextmanager
def isolated_test_dir(label: str) -> Iterator[Path]:
    root = (SCRIPT.parents[1] / ".local-tests").resolve()
    path = (root / f"{label}-{uuid.uuid4().hex}").resolve()
    if not path.is_relative_to(root):
        raise AssertionError("测试目录逃逸 .local-tests")
    path.mkdir(parents=True)
    try:
        yield path
    finally:
        if path.exists():
            shutil.rmtree(path, onerror=remove_readonly)


def remove_readonly(function: object, path: str, _: object) -> None:
    os.chmod(path, stat.S_IWRITE)
    function(path)  # type: ignore[operator]


class PythonEnvironmentTests(unittest.TestCase):
    def test_reads_exact_versions_from_hashed_requirements(self) -> None:
        with isolated_test_dir("python-environment") as directory:
            path = directory / "requirements.txt"
            path.write_text(
                "first-package==1.2.3 \\\n    --hash=sha256:abc\nsecond==4.5.6\n",
                encoding="utf-8",
            )
            errors: list[str] = []

            requirements = MODULE.locked_requirements(path, errors)

        self.assertEqual(
            requirements,
            {"first-package": "1.2.3", "second": "4.5.6"},
        )
        self.assertEqual(errors, [])

    def test_reports_missing_and_mismatched_distributions(self) -> None:
        errors: list[str] = []
        with patch.object(
            MODULE.importlib.metadata,
            "version",
            side_effect=["1.0", MODULE.importlib.metadata.PackageNotFoundError],
        ):
            MODULE.validate_distributions(
                {"mismatch": "2.0", "missing": "3.0"}, errors
            )

        self.assertTrue(any("版本不匹配" in error for error in errors))
        self.assertTrue(any("缺少固定" in error for error in errors))

    def test_native_probe_repeats_real_tree_sitter_parse(self) -> None:
        self.assertEqual(MODULE.native_probe(), 0)

    def test_parent_reports_native_process_failure(self) -> None:
        completed = subprocess.CompletedProcess(
            args=["python"], returncode=0xC0000005, stdout="", stderr=""
        )
        with (
            patch.object(MODULE, "validate_environment", return_value=[]),
            patch.object(MODULE, "run_native_probe", return_value=completed),
            patch.object(MODULE.sys, "argv", [str(SCRIPT)]),
        ):
            self.assertEqual(MODULE.main(), 1)


if __name__ == "__main__":
    unittest.main()

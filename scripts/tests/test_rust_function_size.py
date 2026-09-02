from __future__ import annotations

import datetime as dt
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


SCRIPT = Path(__file__).resolve().parents[1] / "rust_function_size.py"
SPEC = importlib.util.spec_from_file_location("rust_function_size", SCRIPT)
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
    path.mkdir()
    try:
        yield path
    finally:
        if path.exists():
            shutil.rmtree(path, onerror=remove_readonly)


def remove_readonly(function: object, path: str, _: object) -> None:
    os.chmod(path, stat.S_IWRITE)
    function(path)  # type: ignore[operator]


def policy(**overrides: object):
    values = {
        "mode": "changed",
        "public_max_lines": 6,
        "private_max_lines": 4,
        "orchestration_max_lines": 3,
        "max_exception_days": 30,
        "orchestrations": [],
        "exceptions": [],
    }
    values.update(overrides)
    errors: list[str] = []
    parsed = MODULE.parse_policy(values, errors)
    if errors:
        raise AssertionError(errors)
    assert parsed is not None
    return parsed


class RustFunctionAstTests(unittest.TestCase):
    def test_runtime_uses_the_native_versions_verified_by_ci(self) -> None:
        errors: list[str] = []

        self.assertTrue(MODULE.validate_parser_versions(errors))
        self.assertEqual(errors, [])

    def test_parser_only_counts_concrete_functions_and_qualifies_methods(self) -> None:
        source = b"""
// fn fake() {}
pub trait Port { fn declaration(&self); }
impl Port for Service {
    pub fn execute(&self) {
        let text = "fn fake_in_string() {}";
    }
}
fn helper() {
    let value = 1;
}
"""
        try:
            from tree_sitter import Language, Parser
            import tree_sitter_rust
        except ImportError as error:  # pragma: no cover - CI 必须安装固定依赖
            self.fail(f"缺少 tree-sitter 测试依赖：{error}")
        language = Language(tree_sitter_rust.language())
        parser = Parser(language)
        tree = parser.parse(source)

        records = MODULE.functions_in_tree("src/lib.rs", source, tree.root_node)

        self.assertEqual(
            [(item.symbol, item.private) for item in records],
            [("Port for Service::execute", False), ("helper", True)],
        )

    def test_changed_mode_only_checks_function_intersecting_new_lines(self) -> None:
        records = [
            MODULE.FunctionRecord("src/lib.rs", "old", 1, 10, 10, True),
            MODULE.FunctionRecord("src/lib.rs", "changed", 20, 30, 11, True),
        ]
        errors: list[str] = []

        checked, violations = MODULE.validate_functions(
            policy(), records, {"src/lib.rs": [(25, 25)]}, errors
        )

        self.assertEqual((checked, violations), (1, 1))
        self.assertEqual(len(errors), 1)
        self.assertIn("changed", errors[0])

    def test_orchestration_limit_uses_exact_path_and_symbol(self) -> None:
        records = [
            MODULE.FunctionRecord("src/lib.rs", "run", 1, 4, 4, False),
            MODULE.FunctionRecord("src/other.rs", "run", 1, 4, 4, False),
        ]
        configured = policy(
            orchestrations=[{"path": "src/lib.rs", "symbol": "run"}]
        )
        errors: list[str] = []

        _, violations = MODULE.validate_functions(configured, records, None, errors)

        self.assertEqual(violations, 1)
        self.assertIn("编排函数", errors[0])

    def test_exception_rejects_growth_and_stale_registration(self) -> None:
        configured = policy(
            exceptions=[
                {
                    "path": "src/lib.rs",
                    "symbol": "legacy",
                    "current_lines": 8,
                    "reason": "拆分中的历史函数",
                    "expires": dt.date.today() + dt.timedelta(days=10),
                },
                {
                    "path": "src/lib.rs",
                    "symbol": "removed",
                    "current_lines": 9,
                    "reason": "等待删除",
                    "expires": dt.date.today() + dt.timedelta(days=10),
                },
            ]
        )
        records = [MODULE.FunctionRecord("src/lib.rs", "legacy", 1, 9, 9, True)]
        errors: list[str] = []

        _, violations = MODULE.validate_functions(configured, records, None, errors)

        self.assertEqual(violations, 1)
        self.assertTrue(any("只能缩短" in error for error in errors))
        self.assertTrue(any("例外已失效" in error for error in errors))

    def test_all_mode_requires_empty_exception_registry(self) -> None:
        raw = {
            "mode": "all",
            "public_max_lines": 150,
            "private_max_lines": 100,
            "orchestration_max_lines": 80,
            "max_exception_days": 30,
            "orchestrations": [],
            "exceptions": [
                {
                    "path": "src/lib.rs",
                    "symbol": "legacy",
                    "current_lines": 151,
                    "reason": "等待拆分的存量函数",
                    "expires": dt.date.today() + dt.timedelta(days=10),
                }
            ],
        }
        errors: list[str] = []

        parsed = MODULE.parse_policy(raw, errors)

        self.assertIsNotNone(parsed)
        self.assertTrue(any("exceptions 清零" in error for error in errors))
        raw["exceptions"] = []
        errors = []
        parsed = MODULE.parse_policy(raw, errors)
        self.assertIsNotNone(parsed)
        self.assertEqual(errors, [])


class RustChangedRangeTests(unittest.TestCase):
    def test_parses_added_and_modified_ranges_but_ignores_deletion_only_hunks(self) -> None:
        diff = """
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -2 +2,3 @@
+one
+two
+three
@@ -10,2 +12,0 @@
-old
-old
"""

        self.assertEqual(MODULE.parse_changed_ranges(diff), {"src/lib.rs": [(2, 4)]})

    def test_changed_mode_includes_every_function_from_untracked_rust_files(self) -> None:
        with isolated_test_dir("rust-function-size") as root:
            subprocess.run(["git", "init", "--quiet"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "ci@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "CI"], cwd=root, check=True
            )
            (root / "tracked.rs").write_text("fn tracked() {}\n", encoding="utf-8")
            subprocess.run(["git", "add", "tracked.rs"], cwd=root, check=True)
            subprocess.run(
                ["git", "commit", "--quiet", "-m", "fixture"], cwd=root, check=True
            )
            source = root / "src" / "new.rs"
            source.parent.mkdir()
            source.write_text("fn first() {}\nfn second() {}\n", encoding="utf-8")

            errors: list[str] = []
            with patch.dict(os.environ, {}, clear=False):
                os.environ.pop("RYFRAME_CI_BASE_SHA", None)
                os.environ.pop("GITHUB_BASE_SHA", None)
                changed = MODULE.changed_line_ranges(root, errors)

        self.assertEqual(errors, [])
        self.assertIsNotNone(changed)
        assert changed is not None
        self.assertEqual(changed["src/new.rs"], [(1, 2**31 - 1)])
        records = [
            MODULE.FunctionRecord("src/new.rs", "first", 1, 1, 1, True),
            MODULE.FunctionRecord("src/new.rs", "second", 2, 2, 1, True),
        ]
        checked, violations = MODULE.validate_functions(
            policy(), records, changed, errors
        )
        self.assertEqual((checked, violations), (2, 0))

    def test_policy_rejects_exception_beyond_thirty_days(self) -> None:
        errors: list[str] = []
        MODULE.parse_policy(
            {
                "mode": "changed",
                "public_max_lines": 150,
                "private_max_lines": 100,
                "orchestration_max_lines": 80,
                "max_exception_days": 30,
                "orchestrations": [],
                "exceptions": [
                    {
                        "path": "src/lib.rs",
                        "symbol": "legacy",
                        "current_lines": 151,
                        "reason": "历史函数",
                        "expires": dt.date.today() + dt.timedelta(days=31),
                    }
                ],
            },
            errors,
        )

        self.assertTrue(any("不得超过 30 天" in error for error in errors))


if __name__ == "__main__":
    unittest.main()

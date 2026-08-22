from __future__ import annotations

import importlib.util
import shutil
import sys
import unittest
import uuid
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_architecture.py"
SPEC = importlib.util.spec_from_file_location("check_architecture", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)
TEST_ROOT = SCRIPT.parents[1] / "target" / "script-tests"
TEST_ROOT.mkdir(parents=True, exist_ok=True)


class PersistenceFutureGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"architecture-{uuid.uuid4().hex}"
        self.root.mkdir()
        self.source_root = self.root / "crates/example/src"
        self.source_root.mkdir(parents=True)
        (self.source_root / "lib.rs").write_text("pub mod standard;\n", encoding="utf-8")
        (self.source_root / "standard.rs").write_text("pub trait Port {}\n", encoding="utf-8")
        self.complex_root = self.source_root / "complex"
        self.complex_root.mkdir()
        (self.complex_root / "legacy.rs").write_text(
            "use crate::PersistenceFuture;\n", encoding="utf-8"
        )
        (self.complex_root / "modern.rs").write_text(
            "pub trait ModernPort {}\n", encoding="utf-8"
        )
        self.allowlist = self.root / "allowlist.toml"

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def write_allowlist(self, exceptions: str) -> None:
        self.allowlist.write_text(
            'version = 1\ndescription = "测试"\n\n' + exceptions,
            encoding="utf-8",
        )

    def validate(self) -> list[str]:
        errors: list[str] = []
        MODULE.validate_persistence_future_usage(
            self.root,
            self.source_root.rglob("*.rs"),
            self.allowlist,
            errors,
        )
        return errors

    def test_accepts_active_glob_exception(self) -> None:
        self.write_allowlist(
            "[[exception]]\n"
            'id = "complex"\n'
            'paths = ["crates/example/src/complex/**"]\n'
            'reason = "复杂事务边界"\n'
        )

        self.assertEqual(self.validate(), [])

    def test_rejects_glob_that_matches_no_product_source(self) -> None:
        self.write_allowlist(
            "[[exception]]\n"
            'id = "missing"\n'
            'paths = ["crates/example/src/missing/**"]\n'
            'reason = "复杂事务边界"\n'
        )

        errors = self.validate()

        self.assertTrue(any("未命中产品 Rust 源码" in error for error in errors))
        self.assertTrue(any("不得新增 PersistenceFuture" in error for error in errors))

    def test_rejects_exception_without_remaining_usage(self) -> None:
        self.write_allowlist(
            "[[exception]]\n"
            'id = "stale"\n'
            'paths = ["crates/example/src/complex/modern.rs"]\n'
            'reason = "复杂事务边界"\n'
        )

        errors = self.validate()

        self.assertTrue(any("路径已失效" in error for error in errors))

    def test_rejects_duplicate_exception_path(self) -> None:
        self.write_allowlist(
            "[[exception]]\n"
            'id = "first"\n'
            'paths = ["crates/example/src/complex/**"]\n'
            'reason = "复杂事务边界"\n\n'
            "[[exception]]\n"
            'id = "second"\n'
            'paths = ["crates/example/src/complex/**"]\n'
            'reason = "复杂事务边界"\n'
        )

        errors = self.validate()

        self.assertTrue(any("白名单路径重复" in error for error in errors))

    def test_rejects_unlisted_usage_in_standard_source(self) -> None:
        (self.source_root / "standard.rs").write_text(
            "use crate::PersistenceFuture;\n", encoding="utf-8"
        )
        self.write_allowlist(
            "[[exception]]\n"
            'id = "complex"\n'
            'paths = ["crates/example/src/complex/**"]\n'
            'reason = "复杂事务边界"\n'
        )

        errors = self.validate()

        self.assertTrue(
            any(
                error.endswith("crates/example/src/standard.rs")
                for error in errors
                if "不得新增 PersistenceFuture" in error
            )
        )


if __name__ == "__main__":
    unittest.main()

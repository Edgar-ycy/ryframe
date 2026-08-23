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


class LegacyPersistenceApiGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"architecture-{uuid.uuid4().hex}"
        self.root.mkdir()
        self.source_root = self.root / "crates/example/src"
        self.source_root.mkdir(parents=True)
        (self.source_root / "lib.rs").write_text("pub mod standard;\n", encoding="utf-8")
        (self.source_root / "standard.rs").write_text("pub trait Port {}\n", encoding="utf-8")

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def validate(self) -> list[str]:
        errors: list[str] = []
        MODULE.validate_legacy_persistence_apis(
            self.root,
            self.source_root.rglob("*.rs"),
            errors,
        )
        return errors

    def test_accepts_modern_async_port(self) -> None:
        self.assertEqual(self.validate(), [])

    def test_rejects_removed_future_alias(self) -> None:
        removed_name = "Persistence" + "Future"
        (self.source_root / "standard.rs").write_text(
            f"use crate::{removed_name};\n", encoding="utf-8"
        )
        errors = self.validate()
        self.assertTrue(any(removed_name in error for error in errors))

    def test_rejects_removed_control_transaction(self) -> None:
        removed_name = "Control" + "Transaction"
        (self.source_root / "standard.rs").write_text(
            f"pub trait {removed_name} {{}}\n", encoding="utf-8"
        )
        errors = self.validate()
        self.assertTrue(any(removed_name in error for error in errors))


if __name__ == "__main__":
    unittest.main()

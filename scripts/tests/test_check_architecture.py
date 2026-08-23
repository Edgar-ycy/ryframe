from __future__ import annotations

import hashlib
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


class AsyncPortTraitGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"async-port-{uuid.uuid4().hex}"
        self.source_root = self.root / "crates/example/src"
        self.source_root.mkdir(parents=True)
        self.source = self.source_root / "port.rs"

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def validate(self) -> list[str]:
        errors: list[str] = []
        MODULE.validate_async_port_traits(
            self.root,
            self.source_root.rglob("*.rs"),
            errors,
        )
        return errors

    def test_accepts_direct_async_trait_method(self) -> None:
        self.source.write_text(
            "pub trait Port { async fn execute(&self) -> Result<(), String>; }\n",
            encoding="utf-8",
        )
        self.assertEqual(self.validate(), [])

    def test_rejects_handwritten_future_return(self) -> None:
        self.source.write_text(
            "pub type WorkFuture<'a> = core::pin::Pin<Box<dyn core::future::Future<Output = ()> + 'a>>;\n"
            "pub trait Port { fn execute(&self) -> WorkFuture<'_>; }\n",
            encoding="utf-8",
        )
        errors = self.validate()
        self.assertTrue(any("Port::execute" in error for error in errors))


class FrozenMigrationSourceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"frozen-migration-{uuid.uuid4().hex}"
        self.source = (
            self.root
            / "crates/example/src/migration/m20260820_000000_baseline/schema.rs"
        )
        self.source.parent.mkdir(parents=True)
        self.source.write_text("pub const BASELINE: &str = \"stable\";\n", encoding="utf-8")
        self.lock = self.root / "catalog/migrations.lock.toml"
        self.lock.parent.mkdir(parents=True)

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def write_lock(self, digest: str | None = None) -> None:
        relative = self.source.relative_to(self.root).as_posix()
        digest = digest or hashlib.sha256(self.source.read_bytes()).hexdigest()
        self.lock.write_text(
            "\n".join(
                [
                    "format_version = 1",
                    'frozen_at = "2026-08-20"',
                    "",
                    "[[files]]",
                    f'path = "{relative}"',
                    f'sha256 = "{digest}"',
                    'storage = "control"',
                    'target = "m20260820_000000_baseline"',
                    "",
                ]
            ),
            encoding="utf-8",
        )

    def test_exact_locked_migration_is_eligible_for_size_exclusion(self) -> None:
        self.write_lock()
        errors: list[str] = []
        self.assertEqual(
            MODULE.frozen_migration_sources(
                self.root,
                "catalog/migrations.lock.toml",
                errors,
            ),
            {self.source.relative_to(self.root).as_posix()},
        )
        self.assertEqual(errors, [])

    def test_hash_mismatch_does_not_create_an_exclusion(self) -> None:
        self.write_lock("0" * 64)
        errors: list[str] = []
        self.assertEqual(
            MODULE.frozen_migration_sources(
                self.root,
                "catalog/migrations.lock.toml",
                errors,
            ),
            set(),
        )
        self.assertTrue(any("哈希不匹配" in error for error in errors))


if __name__ == "__main__":
    unittest.main()

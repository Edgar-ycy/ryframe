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


class DocumentationPolicyTests(unittest.TestCase):
    def test_changelog_is_allowed_without_a_history_size_limit(self) -> None:
        self.assertEqual(MODULE.HISTORICAL_DOCUMENTS, {"CHANGELOG.md"})
        self.assertNotIn("CHANGELOG.md", MODULE.DOCUMENT_LIMITS)


class RustSourceDiscoveryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"rust-sources-{uuid.uuid4().hex}"
        self.package = self.root / "crates/example"
        (self.package / "src").mkdir(parents=True)
        (self.package / "templates").mkdir()
        (self.package / "Cargo.toml").write_text("[package]\n", encoding="utf-8")
        (self.package / "src/lib.rs").write_text("pub fn safe() {}\n", encoding="utf-8")
        (self.package / "templates/generated.rs.tpl").write_text(
            "pub fn {name}() {}\n", encoding="utf-8"
        )

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def test_unsafe_policy_discovers_rust_templates_separately(self) -> None:
        profile = {"products": {"example"}, "tools": set()}
        packages = {"example": {"manifest_path": str(self.package / "Cargo.toml")}}

        self.assertEqual(
            MODULE.workspace_rust_templates(self.root, profile, packages),
            [self.package / "templates/generated.rs.tpl"],
        )


class UnsafeLintPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"unsafe-lint-{uuid.uuid4().hex}"
        self.package = self.root / "crates/example"
        self.package.mkdir(parents=True)
        (self.root / "Cargo.toml").write_text(
            '[workspace]\n[workspace.lints.rust]\nunsafe_code = "forbid"\n',
            encoding="utf-8",
        )
        (self.package / "Cargo.toml").write_text(
            '[package]\nname = "example"\nversion = "0.1.0"\n[lints]\nworkspace = true\n',
            encoding="utf-8",
        )
        self.packages = {
            "example": {"manifest_path": str(self.package / "Cargo.toml")}
        }

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def test_accepts_forbid_and_workspace_inheritance(self) -> None:
        errors: list[str] = []

        checked = MODULE.validate_unsafe_lint_policy(self.root, self.packages, errors)

        self.assertEqual(checked, 1)
        self.assertEqual(errors, [])

    def test_rejects_weaker_workspace_lint_and_missing_inheritance(self) -> None:
        (self.root / "Cargo.toml").write_text(
            '[workspace]\n[workspace.lints.rust]\nunsafe_code = "warn"\n',
            encoding="utf-8",
        )
        (self.package / "Cargo.toml").write_text(
            '[package]\nname = "example"\nversion = "0.1.0"\n',
            encoding="utf-8",
        )
        errors: list[str] = []

        MODULE.validate_unsafe_lint_policy(self.root, self.packages, errors)

        self.assertTrue(any("unsafe_code" in error for error in errors))
        self.assertTrue(any("继承" in error for error in errors))


class SystemDomainSurfaceGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"system-domains-{uuid.uuid4().hex}"
        self.system_root = self.root / "crates/ryframe-application/src/system"
        self.system_root.mkdir(parents=True)

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def validate(self, source: str) -> list[str]:
        (self.system_root / "mod.rs").write_text(source, encoding="utf-8")
        errors: list[str] = []
        MODULE.validate_system_domain_surface(self.root, errors)
        return errors

    def test_accepts_only_four_domain_modules(self) -> None:
        source = "\n".join(
            [
                "mod user;",
                "pub mod content;",
                "pub mod identity;",
                "pub mod operations;",
                "pub mod platform;",
            ]
        )
        self.assertEqual(self.validate(source), [])

    def test_rejects_public_leaf_module(self) -> None:
        source = "\n".join(
            [
                "pub mod content;",
                "pub mod identity;",
                "pub mod operations;",
                "pub mod platform;",
                "pub mod user;",
            ]
        )
        self.assertTrue(any("公开模块" in error for error in self.validate(source)))

    def test_rejects_root_compatibility_reexport(self) -> None:
        source = "\n".join(
            [
                "pub mod content;",
                "pub mod identity;",
                "pub mod operations;",
                "pub mod platform;",
                "pub use identity::UserService;",
            ]
        )
        self.assertTrue(any("re-export" in error for error in self.validate(source)))


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


class CrateBoundaryPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"crate-boundaries-{uuid.uuid4().hex}"
        self.root.mkdir()

    def tearDown(self) -> None:
        shutil.rmtree(self.root)

    def packages(self) -> dict[str, dict[str, object]]:
        first = self.root / "crates/first/Cargo.toml"
        second = self.root / "crates/second/Cargo.toml"
        return {
            "first": {
                "manifest_path": str(first),
                "dependencies": [{"path": str(second.parent)}],
            },
            "second": {"manifest_path": str(second), "dependencies": []},
        }

    def profile(self, edges: set[tuple[str, str]]) -> dict[str, object]:
        return {
            "packages": {"first", "second"},
            "products": {"first", "second"},
            "tools": set(),
            "allowed_edges": edges,
            "temporary_edges": set(),
            "expected_count": 2,
        }

    def test_active_edges_are_an_exact_fact_source(self) -> None:
        errors: list[str] = []
        actual = MODULE.validate_active_workspace(
            "final",
            self.profile({("first", "second")}),
            self.packages(),
            errors,
        )

        self.assertEqual(actual, {("first", "second")})
        self.assertEqual(errors, [])

    def test_rejects_declared_edge_that_no_longer_exists(self) -> None:
        errors: list[str] = []
        MODULE.validate_active_workspace(
            "final",
            self.profile({("first", "second"), ("second", "first")}),
            self.packages(),
            errors,
        )

        self.assertTrue(any("已不存在" in error for error in errors))

    def test_temporary_edge_requires_reason_and_expiry(self) -> None:
        errors: list[str] = []
        parsed = MODULE.parse_temporary_edges(
            [{"edge": "first -> tool"}],
            "temporary",
            errors,
        )

        self.assertEqual(parsed, set())
        self.assertTrue(any("缺少字段" in error for error in errors))

    def test_temporary_edge_rejects_expired_registration(self) -> None:
        errors: list[str] = []
        parsed = MODULE.parse_temporary_edges(
            [
                {
                    "edge": "first -> tool",
                    "reason": "等待边界迁移",
                    "expires": "2000-01-01",
                }
            ],
            "temporary",
            errors,
        )

        self.assertEqual(parsed, {("first", "tool")})
        self.assertTrue(any("过期" in error for error in errors))

    def test_temporary_edge_accepts_complete_future_registration(self) -> None:
        errors: list[str] = []
        parsed = MODULE.parse_temporary_edges(
            [
                {
                    "edge": "first -> tool",
                    "reason": "等待边界迁移",
                    "expires": "2099-01-01",
                }
            ],
            "temporary",
            errors,
        )

        self.assertEqual(parsed, {("first", "tool")})
        self.assertEqual(errors, [])


class SourceSizeThresholdTests(unittest.TestCase):
    def test_thresholds_warn_at_eighty_and_ninety_percent(self) -> None:
        self.assertIsNone(MODULE.source_size_level(479, 600))
        self.assertEqual(MODULE.source_size_level(480, 600), "hint")
        self.assertEqual(MODULE.source_size_level(540, 600), "strong")

    def test_hard_limit_itself_fails(self) -> None:
        self.assertEqual(MODULE.source_size_level(599, 600), "strong")
        self.assertEqual(MODULE.source_size_level(600, 600), "fail")


class TestLayoutPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = TEST_ROOT / f"test-layout-{uuid.uuid4().hex}"
        self.package_root = self.root / "crates/example"
        self.source_root = self.package_root / "src"
        self.source_root.mkdir(parents=True)
        self.manifest = self.package_root / "Cargo.toml"
        self.manifest.write_text("[package]\nname = 'example'\nversion = '0.1.0'\n", encoding="utf-8")
        self.source = self.source_root / "lib.rs"
        self.previous_root = MODULE.ROOT
        MODULE.ROOT = self.root

    def tearDown(self) -> None:
        MODULE.ROOT = self.previous_root
        shutil.rmtree(self.root)

    def validate(
        self,
        targets: list[dict[str, object]] | None = None,
        disabled_bin_test_targets: list[str] | None = None,
    ) -> list[str]:
        errors: list[str] = []
        MODULE.validate_test_layout(
            {
                "directory": "tests",
                "allow_colocated_unit_tests": True,
                "forbid_source_test_files": True,
                "max_integration_test_lines": 1000,
                "disabled_bin_test_targets": disabled_bin_test_targets or [],
            },
            {
                "example": {
                    "manifest_path": str(self.manifest),
                    "targets": targets
                    or [
                        {
                            "kind": ["lib"],
                            "src_path": str(self.source),
                        }
                    ],
                }
            },
            errors,
        )
        return errors

    def test_accepts_colocated_private_unit_tests(self) -> None:
        self.source.write_text(
            "fn normalize() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn normalizes() { super::normalize(); }\n}\n",
            encoding="utf-8",
        )

        self.assertEqual(self.validate(), [])

    def test_rejects_a_separate_test_module_under_src(self) -> None:
        self.source.write_text("pub mod model_tests;\n", encoding="utf-8")
        (self.source_root / "model_tests.rs").write_text(
            "#[test]\nfn checks_model() {}\n", encoding="utf-8"
        )

        errors = self.validate()

        self.assertTrue(any("model_tests.rs" in error for error in errors))

    def test_accepts_disabled_empty_binary_harness(self) -> None:
        self.source.write_text("fn main() {}\n", encoding="utf-8")
        target = {
            "kind": ["bin"],
            "name": "tool",
            "src_path": str(self.source),
            "test": False,
        }

        self.assertEqual(self.validate([target], ["example:tool"]), [])

    def test_rejects_tests_inside_disabled_binary_harness(self) -> None:
        self.source.write_text(
            "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn checks() {}\n}\n",
            encoding="utf-8",
        )
        target = {
            "kind": ["bin"],
            "name": "tool",
            "src_path": str(self.source),
            "test": False,
        }

        errors = self.validate([target], ["example:tool"])

        self.assertTrue(any("不得包含单测" in error for error in errors))

    def test_rejects_binary_with_default_test_harness(self) -> None:
        self.source.write_text("fn main() {}\n", encoding="utf-8")
        target = {
            "kind": ["bin"],
            "name": "tool",
            "src_path": str(self.source),
            "test": True,
        }

        errors = self.validate([target], ["example:tool"])

        self.assertTrue(any("test = false" in error for error in errors))

    def test_accepts_binary_with_real_test_harness(self) -> None:
        self.source.write_text(
            "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn checks() {}\n}\n",
            encoding="utf-8",
        )
        target = {
            "kind": ["bin"],
            "name": "tool",
            "src_path": str(self.source),
            "test": True,
        }

        self.assertEqual(self.validate([target]), [])

    def test_rejects_unlisted_disabled_binary_harness(self) -> None:
        self.source.write_text("fn main() {}\n", encoding="utf-8")
        target = {
            "kind": ["bin"],
            "name": "tool",
            "src_path": str(self.source),
            "test": False,
        }

        errors = self.validate([target])

        self.assertTrue(any("必须登记" in error for error in errors))


if __name__ == "__main__":
    unittest.main()

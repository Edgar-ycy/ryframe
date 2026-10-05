from __future__ import annotations

import hashlib
import importlib.util
import os
import shutil
import stat
import subprocess
import sys
import unittest
import uuid
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))
SCRIPT = Path(__file__).resolve().parent / "check_migration_history.py"
SPEC = importlib.util.spec_from_file_location("check_migration_history", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class MigrationHistoryTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary_root = Path.cwd() / "target"
        temporary_root.mkdir(exist_ok=True)
        self.root = temporary_root / (
            f"migration-history-test-{os.getpid()}-{uuid.uuid4().hex}"
        )
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "1.0.0"\n', encoding="utf-8"
        )
        self.control = self.root / "crates/ryframe-db/src/migration"
        self.tenant = self.root / "crates/ryframe-tenant-db/src/migration"
        self.catalog = self.root / "catalog"
        self.control.mkdir(parents=True)
        self.tenant.mkdir(parents=True)
        self.catalog.mkdir()

        (self.control.parent / "lib.rs").write_text(
            "pub mod generated;\npub mod migration;\n", encoding="utf-8"
        )
        control_generated = self.control.parent / "generated/mod.rs"
        control_generated.parent.mkdir()
        control_generated.write_text(
            "pub const MIGRATION_NAMES: &[&str] = &[];\n"
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
            encoding="utf-8",
        )
        (self.tenant.parent / "lib.rs").write_text(
            "pub mod generated;\npub mod migration;\n", encoding="utf-8"
        )
        tenant_generated = self.tenant.parent / "generated/mod.rs"
        tenant_generated.parent.mkdir()
        tenant_generated.write_text(
            "pub const MIGRATION_NAMES: &[&str] = &[];\n"
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
            encoding="utf-8",
        )

        control_baseline = self.control / "m20260820_000000_control_baseline"
        control_baseline.mkdir()
        (control_baseline / "mod.rs").write_text("baseline\n", encoding="utf-8")
        (self.tenant / "m20260820_000000_tenant_baseline.rs").write_text(
            "baseline\n", encoding="utf-8"
        )
        (self.control / "mod.rs").write_text(
            "mod m20260820_000000_control_baseline;\n"
            "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n"
            '    "m20260820_000000_control_baseline",\n'
            "];\n"
            "pub fn expected_migration_names() {}\n"
            "m20260820_000000_control_baseline::Migration\n"
            "migrations.extend(crate::generated::migrations());\n",
            encoding="utf-8",
        )
        (self.tenant / "mod.rs").write_text(
            "mod m20260820_000000_tenant_baseline;\n"
            "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n"
            '    "m20260820_000000_tenant_baseline",\n'
            "];\n"
            "pub fn expected_migration_names() {}\n",
            encoding="utf-8",
        )
        (self.tenant / "runtime.rs").write_text(
            "m20260820_000000_tenant_baseline::Migration\n"
            "migrations.extend(crate::generated::migrations());\n",
            encoding="utf-8",
        )
        self._write_lock(
            [
                control_baseline / "mod.rs",
                self.tenant / "m20260820_000000_tenant_baseline.rs",
            ]
        )

    def tearDown(self) -> None:
        def remove_readonly(function, path: str, _error) -> None:
            os.chmod(path, stat.S_IWRITE)
            function(path)

        shutil.rmtree(self.root, onexc=remove_readonly)

    def _write_lock(self, paths: list[Path]) -> None:
        lines = ["format_version = 1", 'frozen_at = "2026-08-20"', ""]
        for path in sorted(paths):
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            relative = path.relative_to(self.root).as_posix()
            storage, target = MODULE.lock_identity(relative)
            lines.extend(
                [
                    "[[files]]",
                    f'path = "{relative}"',
                    f'sha256 = "{digest}"',
                    f'storage = "{storage}"',
                    f'target = "{target}"',
                    "",
                ]
            )
        (self.catalog / "migrations.lock.toml").write_text(
            "\n".join(lines), encoding="utf-8"
        )

    def _create_forward(self, name: str, *, tenant: bool = False) -> Path:
        source = (
            self.tenant / f"{name}.rs"
            if tenant
            else self.control / f"{name}.rs"
        )
        source.write_text(
            "async fn up() { Ok(()) }\n"
            'async fn down() { Err(DbErr::Custom("追加迁移只允许 roll-forward".into())) }\n',
            encoding="utf-8",
        )
        if tenant:
            (self.tenant / "mod.rs").write_text(
                f"mod m20260820_000000_tenant_baseline;\nmod {name};\n"
                "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n"
                '    "m20260820_000000_tenant_baseline",\n'
                f'    "{name}",\n'
                "];\n"
                "pub fn expected_migration_names() {}\n",
                encoding="utf-8",
            )
            (self.tenant / "runtime.rs").write_text(
                f"m20260820_000000_tenant_baseline::Migration\n{name}::Migration\n"
                "migrations.extend(crate::generated::migrations());\n",
                encoding="utf-8",
            )
        else:
            (self.control / "mod.rs").write_text(
                "mod m20260820_000000_control_baseline;\n"
                f"mod {name};\n"
                "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n"
                '    "m20260820_000000_control_baseline",\n'
                f'    "{name}",\n'
                "];\n"
                "pub fn expected_migration_names() {}\n"
                "m20260820_000000_control_baseline::Migration\n"
                f"{name}::Migration\n"
                "migrations.extend(crate::generated::migrations());\n",
                encoding="utf-8",
            )
        return source

    def _commit_all(self, message: str) -> str:
        if not (self.root / ".git").exists():
            subprocess.run(["git", "init", "-q", str(self.root)], check=True)
            subprocess.run(
                ["git", "-C", str(self.root), "config", "user.name", "migration-test"],
                check=True,
            )
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(self.root),
                    "config",
                    "user.email",
                    "test@example.invalid",
                ],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(self.root), "config", "core.autocrlf", "false"],
                check=True,
            )
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        subprocess.run(
            ["git", "-C", str(self.root), "commit", "-q", "-m", message],
            check=True,
        )
        return subprocess.check_output(
            ["git", "-C", str(self.root), "rev-parse", "HEAD"], text=True
        ).strip()

    def test_frozen_file_change_is_rejected(self) -> None:
        frozen = self.control / "m20260820_000000_control_baseline/mod.rs"
        frozen.write_text("changed\n", encoding="utf-8")

        errors = MODULE.verify_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )

        self.assertTrue(any("冻结迁移被修改" in error for error in errors))

    def test_generated_migrations_must_be_consumed_by_both_runtime_registries(self) -> None:
        runtime = self.tenant / "runtime.rs"
        runtime.write_text(
            "m20260820_000000_tenant_baseline::Migration\n", encoding="utf-8"
        )

        errors = MODULE.verify_generated_registry_wiring(self.root)

        self.assertTrue(any("租户库运行时 Migrator 必须消费" in error for error in errors))

    def test_forward_migration_requires_registration_and_rejected_down(self) -> None:
        name = "m20260823_010203_add_business"
        migration = self.control / name
        migration.mkdir()
        (migration / "mod.rs").write_text(
            "async fn down() { Err(DbErr::Custom(\"production 不支持 down，请追加修复迁移\".into())) }\n",
            encoding="utf-8",
        )
        (self.control / "mod.rs").write_text(
            "mod m20260820_000000_control_baseline;\n"
            f"mod {name};\n"
            "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n"
            '    "m20260820_000000_control_baseline",\n'
            f'    "{name}",\n'
            "];\n"
            "pub fn expected_migration_names() {}\n"
            "m20260820_000000_control_baseline::Migration\n"
            f"{name}::Migration\n",
            encoding="utf-8",
        )

        errors = MODULE.verify_append_only(self.root)

        self.assertEqual(errors, [])

    def test_forward_migration_requires_read_only_name_registration(self) -> None:
        name = "m20260823_010203_add_business"
        self._create_forward(name)
        registry = self.control / "mod.rs"
        registry.write_text(
            registry.read_text(encoding="utf-8").replace(f'    "{name}",\n', ""),
            encoding="utf-8",
        )

        errors = MODULE.verify_append_only(self.root)

        self.assertTrue(any("迁移未加入只读名称注册表" in error for error in errors))

    def test_forward_migration_with_rollback_body_is_rejected(self) -> None:
        name = "m20260823_010203_add_business"
        migration = self.tenant / f"{name}.rs"
        migration.write_text("async fn down() { Ok(()) }\n", encoding="utf-8")
        (self.tenant / "mod.rs").write_text(
            f"mod m20260820_000000_tenant_baseline;\nmod {name};\n",
            encoding="utf-8",
        )
        (self.tenant / "runtime.rs").write_text(
            f"m20260820_000000_tenant_baseline::Migration\n{name}::Migration\n",
            encoding="utf-8",
        )

        errors = MODULE.verify_append_only(self.root)

        self.assertTrue(any("down 必须返回" in error for error in errors))

    def test_down_rejection_text_cannot_hide_a_rollback_body(self) -> None:
        name = "m20260823_010203_add_business"
        migration = self.tenant / f"{name}.rs"
        migration.write_text(
            "async fn down() {\n"
            "    execute_destructive_rollback().await?;\n"
            '    let _fake_guard = "Err(DbErr::Custom(\\\"只允许追加修复\\\".into()))";\n'
            "    Ok(())\n"
            "}\n",
            encoding="utf-8",
        )
        (self.tenant / "mod.rs").write_text(
            f"mod m20260820_000000_tenant_baseline;\nmod {name};\n",
            encoding="utf-8",
        )
        (self.tenant / "runtime.rs").write_text(
            f"m20260820_000000_tenant_baseline::Migration\n{name}::Migration\n",
            encoding="utf-8",
        )

        errors = MODULE.verify_append_only(self.root)

        self.assertTrue(any("不得执行其他逻辑" in error for error in errors))

    def test_pending_migration_is_editable_but_full_check_requires_freeze(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")
        _document, entries, errors = MODULE.load_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )
        self.assertEqual(errors, [])
        locked = {entry.relative for entry in entries}

        self.assertEqual(
            MODULE.verify_append_only(
                self.root, locked, head_paths=set(), require_frozen=False
            ),
            [],
        )
        full_errors = MODULE.verify_append_only(
            self.root, locked, head_paths=set(), require_frozen=True
        )
        self.assertTrue(any("尚未冻结" in error for error in full_errors))
        self.assertTrue(source.is_file())

    def test_committed_unlocked_migration_cannot_be_silently_accepted(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")
        _document, entries, errors = MODULE.load_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )
        self.assertEqual(errors, [])
        locked = {entry.relative for entry in entries}
        relative = source.relative_to(self.root).as_posix()

        errors = MODULE.verify_append_only(
            self.root, locked, head_paths={relative}, require_frozen=False
        )

        self.assertTrue(any("已提交迁移未进入冻结清单" in error for error in errors))

    def test_frozen_forward_migration_change_delete_and_rename_are_rejected(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")
        baselines = [
            self.control / "m20260820_000000_control_baseline/mod.rs",
            self.tenant / "m20260820_000000_tenant_baseline.rs",
        ]
        self._write_lock([*baselines, source])
        lock = self.catalog / "migrations.lock.toml"

        source.write_text("changed\n", encoding="utf-8")
        self.assertTrue(any("冻结迁移被修改" in error for error in MODULE.verify_lock(self.root, lock)))

        self._create_forward("m20260823_010203_add_business")
        source.unlink()
        self.assertTrue(any("被删除或改名" in error for error in MODULE.verify_lock(self.root, lock)))

        renamed = source.with_name("m20260823_010204_add_business.rs")
        renamed.write_text("replacement\n", encoding="utf-8")
        self.assertTrue(any("被删除或改名" in error for error in MODULE.verify_lock(self.root, lock)))

    def test_freeze_records_only_new_complete_migration(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")

        errors = MODULE.freeze(self.root)

        self.assertEqual(errors, [])
        document, entries, load_errors = MODULE.load_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )
        self.assertIsNotNone(document)
        self.assertEqual(load_errors, [])
        self.assertIn(
            source.relative_to(self.root).as_posix(),
            {entry.relative for entry in entries},
        )
        source.write_text("changed after freeze\n", encoding="utf-8")
        self.assertTrue(
            any(
                "冻结迁移被修改" in error
                for error in MODULE.check(self.root, require_frozen=True)
            )
        )

    def test_freeze_rejects_unimplemented_skeleton(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")
        source.write_text(
            "async fn up() { Err(\"尚未实现\") }\n"
            'async fn down() { Err(DbErr::Custom("追加迁移".into())) }\n',
            encoding="utf-8",
        )
        errors = MODULE.freeze(self.root)

        self.assertTrue(any("未实现骨架" in error for error in errors))

    def test_generated_initial_migration_must_be_frozen_before_commit(self) -> None:
        source = self.root / "crates/ryframe-tenant-db/src/generated/business/migration.rs"
        source.parent.mkdir(parents=True)
        source.write_text(
            "pub const INITIAL_RESOURCE_MIGRATION: bool = true;\n"
            "async fn down() { Err(DbErr::Custom(\"只允许追加修复\".into())) }\n",
            encoding="utf-8",
        )
        tenant_generated = self.tenant.parent / "generated/mod.rs"
        tenant_generated.write_text(
            'pub const MIGRATION_NAMES: &[&str] = &["m_resource_initial_business"];\n'
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
            encoding="utf-8",
        )
        _document, entries, load_errors = MODULE.load_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )
        self.assertEqual(load_errors, [])
        locked = {entry.relative for entry in entries}
        relative = source.relative_to(self.root).as_posix()

        pending = MODULE.verify_append_only(
            self.root, locked, head_paths=set(), require_frozen=True
        )
        self.assertTrue(any("待提交迁移尚未冻结" in error for error in pending))
        committed = MODULE.verify_append_only(
            self.root, locked, head_paths={relative}, require_frozen=False
        )
        self.assertTrue(any("已提交迁移未进入冻结清单" in error for error in committed))

        self.assertEqual(MODULE.freeze(self.root), [])
        _document, entries, load_errors = MODULE.load_lock(
            self.root, self.catalog / "migrations.lock.toml"
        )
        self.assertEqual(load_errors, [])
        self.assertIn(relative, {entry.relative for entry in entries})
        source.write_text("changed\n", encoding="utf-8")
        self.assertTrue(
            any("冻结迁移被修改" in error for error in MODULE.verify_lock(
                self.root, self.catalog / "migrations.lock.toml"
            ))
        )

    def test_generated_initial_migration_requires_read_only_name_registration(self) -> None:
        source = self.root / "crates/ryframe-tenant-db/src/generated/business/migration.rs"
        source.parent.mkdir(parents=True)
        source.write_text(
            "pub const INITIAL_RESOURCE_MIGRATION: bool = true;\n"
            "async fn down() { Err(DbErr::Custom(\"只允许追加修复\".into())) }\n",
            encoding="utf-8",
        )

        errors = MODULE.verify_append_only(self.root)

        self.assertTrue(any("generated migration 未加入只读名称注册表" in error for error in errors))

    def test_source_and_existing_lock_hash_cannot_change_together(self) -> None:
        self._commit_all("trusted lock")
        frozen = self.control / "m20260820_000000_control_baseline/mod.rs"
        frozen.write_text("changed together\n", encoding="utf-8")
        self._write_lock(
            [
                frozen,
                self.tenant / "m20260820_000000_tenant_baseline.rs",
            ]
        )

        errors = MODULE.check(self.root)

        self.assertTrue(any("HEAD 条目" in error for error in errors))

    def test_existing_head_migration_cannot_be_appended_to_lock(self) -> None:
        source = self._create_forward("m20260823_010203_add_business")
        baselines = [
            self.control / "m20260820_000000_control_baseline/mod.rs",
            self.tenant / "m20260820_000000_tenant_baseline.rs",
        ]
        self._commit_all("unlocked historical migration")
        self._write_lock([*baselines, source])

        errors = MODULE.check(self.root)

        self.assertTrue(any("拒绝给已存在于 HEAD" in error for error in errors))

    def test_first_lock_bootstrap_accepts_only_unchanged_head_baselines(self) -> None:
        lock = self.catalog / "migrations.lock.toml"
        lock.unlink()
        self._commit_all("baselines before lock")
        baselines = [
            self.control / "m20260820_000000_control_baseline/mod.rs",
            self.tenant / "m20260820_000000_tenant_baseline.rs",
        ]
        self._write_lock(baselines)

        self.assertEqual(MODULE.check(self.root), [])

    def test_explicit_trusted_ref_rejects_tampered_source_and_lock_commit(self) -> None:
        trusted = self._commit_all("trusted baseline")
        frozen = self.control / "m20260820_000000_control_baseline/mod.rs"
        frozen.write_text("tampered in second commit\n", encoding="utf-8")
        self._write_lock(
            [
                frozen,
                self.tenant / "m20260820_000000_tenant_baseline.rs",
            ]
        )
        self._commit_all("tampered source and matching lock")

        errors = MODULE.check(self.root, trusted_ref=trusted)

        self.assertTrue(any("HEAD 条目" in error for error in errors))

    def test_new_lock_entry_cannot_reuse_trusted_storage_and_target(self) -> None:
        self._commit_all("trusted baseline")
        extra = self.control / "m20260820_000000_control_baseline/extra.rs"
        extra.write_text("late historical file\n", encoding="utf-8")
        self._write_lock(
            [
                self.control / "m20260820_000000_control_baseline/mod.rs",
                self.tenant / "m20260820_000000_tenant_baseline.rs",
                extra,
            ]
        )

        errors = MODULE.check(self.root)

        self.assertTrue(any("复用了受信迁移目标" in error for error in errors))

    def test_development_baseline_refresh_is_explicit_and_read_only_check_detects_drift(
        self,
    ):
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.12.1"\n', encoding="utf-8"
        )
        self._commit_all("development baseline")
        source = self.control / "m20260820_000000_control_baseline/mod.rs"
        source.write_text("new development baseline\n", encoding="utf-8")
        lock = self.catalog / "migrations.lock.toml"
        before = lock.read_bytes()
        self.assertTrue(MODULE.check(self.root))
        self.assertEqual(lock.read_bytes(), before)
        self.assertEqual(MODULE.refresh_baseline(self.root), [])
        self.assertNotEqual(lock.read_bytes(), before)
        self.assertEqual(MODULE.check(self.root, require_frozen=True), [])

    def test_stable_baseline_refresh_is_rejected_without_writes(self):
        self._commit_all("stable baseline")
        lock = self.catalog / "migrations.lock.toml"
        before = lock.read_bytes()
        self.assertTrue(MODULE.refresh_baseline(self.root))
        self.assertEqual(lock.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()

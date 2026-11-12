use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use super::migration::{
    check_history, freeze_history, load_lock, lock_identity, parse_lock, refresh_history_baseline,
    run_reconcile, safe_locked_path, sha256, verify_append_only, verify_generated_registry_wiring,
    verify_lock, write_lock_atomically,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/migration-history-rust-tests");
        fs::create_dir_all(&parent).unwrap();
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!("{}-{id}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = Self(root);
        root.write("Cargo.toml", "[workspace.package]\nversion = \"1.0.0\"\n");
        root.write(
            "crates/ryframe-db/src/lib.rs",
            "pub mod generated;\npub mod migration;\n",
        );
        root.write(
            "crates/ryframe-db/src/generated/mod.rs",
            "pub const MIGRATION_NAMES: &[&str] = &[];\n\
             pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
        );
        root.write(
            "crates/ryframe-tenant-db/src/lib.rs",
            "pub mod generated;\npub mod migration;\n",
        );
        root.write(
            "crates/ryframe-tenant-db/src/generated/mod.rs",
            "pub const MIGRATION_NAMES: &[&str] = &[];\n\
             pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
        );
        root.write(
            "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
            "baseline\n",
        );
        root.write(
            "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
            "baseline\n",
        );
        root.write(
            "crates/ryframe-db/src/migration/mod.rs",
            &control_registry(&[]),
        );
        root.write(
            "crates/ryframe-tenant-db/src/migration/mod.rs",
            &tenant_module_registry(&[]),
        );
        root.write(
            "crates/ryframe-tenant-db/src/migration/runtime.rs",
            &tenant_runtime_registry(&[]),
        );
        root.write_lock(&[
            root.path("crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs"),
            root.path("crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs"),
        ]);
        root
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn write_lock(&self, paths: &[PathBuf]) {
        let mut paths = paths.to_vec();
        paths.sort();
        let mut content = "format_version = 1\nfrozen_at = \"2026-08-20\"\n".to_owned();
        for path in paths {
            let relative = path
                .strip_prefix(&self.0)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let (storage, target) = lock_identity(&relative).unwrap();
            content.push_str(&format!(
                "\n[[files]]\npath = \"{relative}\"\nsha256 = \"{}\"\nstorage = \"{storage}\"\ntarget = \"{target}\"\n",
                sha256(&path).unwrap()
            ));
        }
        self.write("catalog/migrations.lock.toml", &content);
    }

    fn create_forward(&self, name: &str, tenant: bool) -> PathBuf {
        let directory = if tenant {
            "crates/ryframe-tenant-db/src/migration"
        } else {
            "crates/ryframe-db/src/migration"
        };
        let relative = format!("{directory}/{name}.rs");
        self.write(
            &relative,
            "async fn up() { Ok(()) }\n\
             async fn down() { Err(DbErr::Custom(\"追加迁移只允许 roll-forward\".into())) }\n",
        );
        if tenant {
            self.write(
                "crates/ryframe-tenant-db/src/migration/mod.rs",
                &tenant_module_registry(&[name]),
            );
            self.write(
                "crates/ryframe-tenant-db/src/migration/runtime.rs",
                &tenant_runtime_registry(&[name]),
            );
        } else {
            self.write(
                "crates/ryframe-db/src/migration/mod.rs",
                &control_registry(&[name]),
            );
        }
        self.path(&relative)
    }

    fn git(&self, arguments: &[&str]) {
        let result = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn commit_all(&self, message: &str) -> String {
        if !self.0.join(".git").exists() {
            self.git(&["init", "-q"]);
            self.git(&["config", "user.name", "migration-test"]);
            self.git(&["config", "user.email", "test@example.invalid"]);
            self.git(&["config", "core.autocrlf", "false"]);
        }
        self.git(&["add", "."]);
        self.git(&["commit", "-q", "-m", message]);
        String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned()
    }

    fn lock(&self) -> PathBuf {
        self.path("catalog/migrations.lock.toml")
    }

    fn baselines(&self) -> [PathBuf; 2] {
        [
            self.path("crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs"),
            self.path("crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs"),
        ]
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn control_registry(extra: &[&str]) -> String {
    let mut content = "mod m20260820_000000_control_baseline;\n".to_owned();
    for name in extra {
        content.push_str(&format!("mod {name};\n"));
    }
    content.push_str(
        "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n\
         \x20   \"m20260820_000000_control_baseline\",\n",
    );
    for name in extra {
        content.push_str(&format!("    \"{name}\",\n"));
    }
    content.push_str(
        "];\n\
         pub fn expected_migration_names() {}\n\
         m20260820_000000_control_baseline::Migration\n",
    );
    for name in extra {
        content.push_str(&format!("{name}::Migration\n"));
    }
    content.push_str("migrations.extend(crate::generated::migrations());\n");
    content
}

fn tenant_module_registry(extra: &[&str]) -> String {
    let mut content = "mod m20260820_000000_tenant_baseline;\n".to_owned();
    for name in extra {
        content.push_str(&format!("mod {name};\n"));
    }
    content.push_str(
        "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[\n\
         \x20   \"m20260820_000000_tenant_baseline\",\n",
    );
    for name in extra {
        content.push_str(&format!("    \"{name}\",\n"));
    }
    content.push_str("];\npub fn expected_migration_names() {}\n");
    content
}

fn tenant_runtime_registry(extra: &[&str]) -> String {
    let mut content = "m20260820_000000_tenant_baseline::Migration\n".to_owned();
    for name in extra {
        content.push_str(&format!("{name}::Migration\n"));
    }
    content.push_str("migrations.extend(crate::generated::migrations());\n");
    content
}

#[test]
fn frozen_file_change_is_rejected() {
    let root = TestRoot::new();
    root.write(
        "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
        "changed\n",
    );
    let errors = verify_lock(&root.0, &root.lock());
    assert!(errors.iter().any(|error| error.contains("冻结迁移被修改")));
}

#[test]
fn locked_paths_stay_inside_the_four_migration_roots() {
    let root = TestRoot::new();
    let valid = "crates/ryframe-db/src/migration/m20260823_010203_add_business/mod.rs";
    assert_eq!(safe_locked_path(&root.0, valid).unwrap().0, valid);
    for invalid in [
        "../outside.rs",
        "/absolute.rs",
        "crates/ryframe-db/src/migration/../escape.rs",
        "crates/ryframe-db/src/migration/m20260823_010203_add_business/C:/escape.rs",
        "crates/ryframe-db/src/migration/m20260823_010203_add_business/note.txt",
        "elsewhere/m20260823_010203_add_business.rs",
    ] {
        assert!(safe_locked_path(&root.0, invalid).is_err(), "{invalid}");
    }
}

#[test]
fn lock_parser_rejects_duplicate_files_and_forged_identity() {
    let root = TestRoot::new();
    let relative = "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs";
    let digest = sha256(&root.baselines()[0]).unwrap();
    let entry = |storage: &str, target: &str| {
        format!(
            "[[files]]\npath = \"{relative}\"\nsha256 = \"{digest}\"\nstorage = \"{storage}\"\ntarget = \"{target}\"\n\n"
        )
    };
    let duplicate = format!(
        "format_version = 1\nfrozen_at = \"2026-08-20\"\n\n{}{}",
        entry("control", "m20260820_000000_control_baseline"),
        entry("control", "m20260820_000000_control_baseline")
    );
    let (_document, _entries, errors) = parse_lock(&root.0, "fixture", duplicate.as_bytes());
    assert!(errors.iter().any(|error| error.contains("冻结文件重复")));

    let forged = format!(
        "format_version = 1\nfrozen_at = \"2026-08-20\"\n\n{}",
        entry("tenant-data", "m20260820_000000_control_baseline")
    );
    let (_document, _entries, errors) = parse_lock(&root.0, "fixture", forged.as_bytes());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("storage 必须是从路径推导"))
    );
}

#[test]
fn atomic_lock_write_refuses_a_stale_preimage_and_cleans_staging() {
    let root = TestRoot::new();
    let lock_path = root.lock();
    let before = fs::read(&lock_path).unwrap();
    let error = write_lock_atomically(&lock_path, b"stale", b"replacement").unwrap_err();
    assert!(error.contains("写入前迁移冻结清单发生变化"));
    assert_eq!(fs::read(&lock_path).unwrap(), before);
    assert!(
        fs::read_dir(lock_path.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".freeze-"))
    );
}

#[test]
fn generated_migrations_must_be_consumed_by_both_runtime_registries() {
    let root = TestRoot::new();
    root.write(
        "crates/ryframe-tenant-db/src/migration/runtime.rs",
        "m20260820_000000_tenant_baseline::Migration\n",
    );
    let errors = verify_generated_registry_wiring(&root.0);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("租户库运行时 Migrator 必须消费"))
    );
}

#[test]
fn forward_migration_requires_registration_and_rejected_down() {
    let root = TestRoot::new();
    root.write(
        "crates/ryframe-db/src/migration/m20260823_010203_add_business/mod.rs",
        "async fn down() { Err(DbErr::Custom(\"production 不支持 down，请追加修复迁移\".into())) }\n",
    );
    root.write(
        "crates/ryframe-db/src/migration/mod.rs",
        &control_registry(&["m20260823_010203_add_business"]),
    );
    assert_eq!(
        verify_append_only(&root.0, None, false, None),
        Vec::<String>::new()
    );
}

#[test]
fn forward_migration_requires_read_only_name_registration() {
    let root = TestRoot::new();
    root.create_forward("m20260823_010203_add_business", false);
    let registry = root.path("crates/ryframe-db/src/migration/mod.rs");
    let source = fs::read_to_string(&registry)
        .unwrap()
        .replace("    \"m20260823_010203_add_business\",\n", "");
    fs::write(registry, source).unwrap();
    let errors = verify_append_only(&root.0, None, false, None);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("未加入只读名称注册表"))
    );
}

#[test]
fn forward_migration_with_rollback_body_is_rejected() {
    let root = TestRoot::new();
    root.create_forward("m20260823_010203_add_business", true);
    root.write(
        "crates/ryframe-tenant-db/src/migration/m20260823_010203_add_business.rs",
        "async fn down() { Ok(()) }\n",
    );
    let errors = verify_append_only(&root.0, None, false, None);
    assert!(errors.iter().any(|error| error.contains("down 必须返回")));
}

#[test]
fn down_rejection_text_cannot_hide_a_rollback_body() {
    let root = TestRoot::new();
    root.create_forward("m20260823_010203_add_business", true);
    root.write(
        "crates/ryframe-tenant-db/src/migration/m20260823_010203_add_business.rs",
        "async fn down() {\n\
         \x20   execute_destructive_rollback().await?;\n\
         \x20   let _fake_guard = \"Err(DbErr::Custom(\\\"只允许追加修复\\\".into()))\";\n\
         \x20   Ok(())\n\
         }\n",
    );
    let errors = verify_append_only(&root.0, None, false, None);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("不得执行其他逻辑"))
    );
}

#[test]
fn pending_migration_is_editable_but_full_check_requires_freeze() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    let (_document, entries, errors) = load_lock(&root.0, &root.lock());
    assert!(errors.is_empty());
    let locked = entries.iter().map(|entry| entry.relative.clone()).collect();
    let empty = std::collections::BTreeSet::new();
    assert!(verify_append_only(&root.0, Some(&locked), false, Some(&empty)).is_empty());
    let errors = verify_append_only(&root.0, Some(&locked), true, Some(&empty));
    assert!(errors.iter().any(|error| error.contains("尚未冻结")));
    assert!(source.is_file());
}

#[test]
fn committed_unlocked_migration_cannot_be_silently_accepted() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    let (_document, entries, _errors) = load_lock(&root.0, &root.lock());
    let locked = entries.iter().map(|entry| entry.relative.clone()).collect();
    let relative = source
        .strip_prefix(&root.0)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let head = [relative].into_iter().collect();
    let errors = verify_append_only(&root.0, Some(&locked), false, Some(&head));
    assert!(
        errors
            .iter()
            .any(|error| error.contains("已提交迁移未进入冻结清单"))
    );
}

#[test]
fn frozen_forward_migration_change_delete_and_rename_are_rejected() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    root.write_lock(&[
        root.baselines()[0].clone(),
        root.baselines()[1].clone(),
        source.clone(),
    ]);
    fs::write(&source, "changed\n").unwrap();
    assert!(
        verify_lock(&root.0, &root.lock())
            .iter()
            .any(|error| error.contains("冻结迁移被修改"))
    );
    root.create_forward("m20260823_010203_add_business", false);
    fs::remove_file(&source).unwrap();
    assert!(
        verify_lock(&root.0, &root.lock())
            .iter()
            .any(|error| error.contains("被删除或改名"))
    );
    fs::write(
        source.with_file_name("m20260823_010204_add_business.rs"),
        "replacement\n",
    )
    .unwrap();
    assert!(
        verify_lock(&root.0, &root.lock())
            .iter()
            .any(|error| error.contains("被删除或改名"))
    );
}

#[test]
fn freeze_records_only_new_complete_migration() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    assert!(freeze_history(&root.0).is_empty());
    let (_document, entries, errors) = load_lock(&root.0, &root.lock());
    assert!(errors.is_empty());
    let relative = source
        .strip_prefix(&root.0)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert!(entries.iter().any(|entry| entry.relative == relative));
    fs::write(source, "changed after freeze\n").unwrap();
    assert!(
        check_history(&root.0, true, "HEAD")
            .iter()
            .any(|error| error.contains("冻结迁移被修改"))
    );
}

#[test]
fn freeze_rejects_unimplemented_skeleton() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    fs::write(
        source,
        "async fn up() { Err(\"尚未实现\") }\n\
         async fn down() { Err(DbErr::Custom(\"追加迁移\".into())) }\n",
    )
    .unwrap();
    assert!(
        freeze_history(&root.0)
            .iter()
            .any(|error| error.contains("未实现骨架"))
    );
}

#[test]
fn generated_initial_migration_must_be_frozen_before_commit() {
    let root = TestRoot::new();
    let source = root.path("crates/ryframe-tenant-db/src/generated/business/migration.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(
        &source,
        "pub const INITIAL_RESOURCE_MIGRATION: bool = true;\n\
         async fn down() { Err(DbErr::Custom(\"只允许追加修复\".into())) }\n",
    )
    .unwrap();
    root.write(
        "crates/ryframe-tenant-db/src/generated/mod.rs",
        "pub const MIGRATION_NAMES: &[&str] = &[\"m_resource_initial_business\"];\n\
         pub fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![] }\n",
    );
    let (_document, entries, _errors) = load_lock(&root.0, &root.lock());
    let locked = entries.iter().map(|entry| entry.relative.clone()).collect();
    let empty = std::collections::BTreeSet::new();
    assert!(
        verify_append_only(&root.0, Some(&locked), true, Some(&empty))
            .iter()
            .any(|error| error.contains("尚未冻结"))
    );
    let relative = source
        .strip_prefix(&root.0)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let head = [relative.clone()].into_iter().collect();
    assert!(
        verify_append_only(&root.0, Some(&locked), false, Some(&head))
            .iter()
            .any(|error| error.contains("已提交迁移未进入冻结清单"))
    );
    assert!(freeze_history(&root.0).is_empty());
    let (_document, entries, errors) = load_lock(&root.0, &root.lock());
    assert!(errors.is_empty());
    assert!(entries.iter().any(|entry| entry.relative == relative));
    fs::write(source, "changed\n").unwrap();
    assert!(
        verify_lock(&root.0, &root.lock())
            .iter()
            .any(|error| error.contains("冻结迁移被修改"))
    );
}

#[test]
fn generated_initial_migration_requires_read_only_name_registration() {
    let root = TestRoot::new();
    root.write(
        "crates/ryframe-tenant-db/src/generated/business/migration.rs",
        "pub const INITIAL_RESOURCE_MIGRATION: bool = true;\n\
         async fn down() { Err(DbErr::Custom(\"只允许追加修复\".into())) }\n",
    );
    assert!(
        verify_append_only(&root.0, None, false, None)
            .iter()
            .any(|error| error.contains("generated migration 未加入只读名称注册表"))
    );
}

#[test]
fn source_and_existing_lock_hash_cannot_change_together() {
    let root = TestRoot::new();
    root.commit_all("trusted lock");
    root.write(
        "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
        "changed together\n",
    );
    root.write_lock(&root.baselines());
    assert!(
        check_history(&root.0, false, "HEAD")
            .iter()
            .any(|error| error.contains("HEAD 条目"))
    );
}

#[test]
fn existing_head_migration_cannot_be_appended_to_lock() {
    let root = TestRoot::new();
    let source = root.create_forward("m20260823_010203_add_business", false);
    root.commit_all("unlocked historical migration");
    root.write_lock(&[
        root.baselines()[0].clone(),
        root.baselines()[1].clone(),
        source,
    ]);
    assert!(
        check_history(&root.0, false, "HEAD")
            .iter()
            .any(|error| error.contains("拒绝给已存在于 HEAD"))
    );
}

#[test]
fn first_lock_bootstrap_accepts_only_unchanged_head_baselines() {
    let root = TestRoot::new();
    fs::remove_file(root.lock()).unwrap();
    root.commit_all("baselines before lock");
    let baselines = root.baselines();
    root.write_lock(&baselines);
    assert_eq!(check_history(&root.0, false, "HEAD"), Vec::<String>::new());
}

#[test]
fn explicit_trusted_ref_rejects_tampered_source_and_lock_commit() {
    let root = TestRoot::new();
    let trusted = root.commit_all("trusted baseline");
    root.write(
        "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
        "tampered in second commit\n",
    );
    root.write_lock(&root.baselines());
    root.commit_all("tampered source and matching lock");
    assert!(
        check_history(&root.0, false, &trusted)
            .iter()
            .any(|error| error.contains("HEAD 条目"))
    );
}

#[test]
fn new_lock_entry_cannot_reuse_trusted_storage_and_target() {
    let root = TestRoot::new();
    root.commit_all("trusted baseline");
    let extra =
        root.path("crates/ryframe-db/src/migration/m20260820_000000_control_baseline/extra.rs");
    fs::write(&extra, "late historical file\n").unwrap();
    root.write_lock(&[
        root.baselines()[0].clone(),
        root.baselines()[1].clone(),
        extra,
    ]);
    assert!(
        check_history(&root.0, false, "HEAD")
            .iter()
            .any(|error| error.contains("复用了受信迁移目标"))
    );
}

#[test]
fn development_baseline_refresh_is_explicit_and_read_only_check_detects_drift() {
    let root = TestRoot::new();
    root.write("Cargo.toml", "[workspace.package]\nversion = \"0.12.1\"\n");
    root.commit_all("development baseline");
    root.write(
        "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
        "new development baseline\n",
    );
    let before = fs::read(root.lock()).unwrap();
    assert!(!check_history(&root.0, false, "HEAD").is_empty());
    assert_eq!(fs::read(root.lock()).unwrap(), before);
    assert!(refresh_history_baseline(&root.0).is_empty());
    assert_ne!(fs::read(root.lock()).unwrap(), before);
    assert!(check_history(&root.0, true, "HEAD").is_empty());
}

#[test]
fn stable_baseline_refresh_is_rejected_without_writes() {
    let root = TestRoot::new();
    root.commit_all("stable baseline");
    let before = fs::read(root.lock()).unwrap();
    assert!(!refresh_history_baseline(&root.0).is_empty());
    assert_eq!(fs::read(root.lock()).unwrap(), before);
}

#[test]
fn reconcile_selects_baseline_or_freeze_from_the_shared_release_stage() {
    let stable = TestRoot::new();
    let source = stable.create_forward("m20260823_010203_add_business", false);
    run_reconcile(&stable.0).unwrap();
    let (_document, entries, errors) = load_lock(&stable.0, &stable.lock());
    assert!(errors.is_empty());
    let relative = source
        .strip_prefix(&stable.0)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert!(entries.iter().any(|entry| entry.relative == relative));

    let development = TestRoot::new();
    development.write("Cargo.toml", "[workspace.package]\nversion = \"0.12.1\"\n");
    development.commit_all("development baseline");
    let before = fs::read(development.lock()).unwrap();
    run_reconcile(&development.0).unwrap();
    assert_eq!(fs::read(development.lock()).unwrap(), before);

    development.write(
        "crates/ryframe-db/src/migration/m20260820_000000_control_baseline/mod.rs",
        "refreshed development baseline\n",
    );
    run_reconcile(&development.0).unwrap();
    let after = fs::read(development.lock()).unwrap();
    assert_ne!(after, before);
    assert!(after.ends_with(b"\n") && !after.ends_with(b"\n\n"));
    assert!(check_history(&development.0, true, "HEAD").is_empty());
}

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    cli::{MigrationOperation, MigrationScope, MigrationTarget},
    migration::{
        FileOperations, PlannedWrite, commit_writes_with, create_migration, migration_run_args,
    },
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

#[test]
fn migration_runner_selects_only_the_migrate_binary_feature() {
    let prefix = [
        "run",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
        "--",
    ];
    let cases = [
        (
            MigrationOperation::Verify,
            MigrationTarget::Control,
            vec!["control", "verify"],
        ),
        (
            MigrationOperation::Status,
            MigrationTarget::TenantDataAll,
            vec!["tenant-data", "status", "--all"],
        ),
        (
            MigrationOperation::Up,
            MigrationTarget::TenantDataOne("tenant-a".to_owned()),
            vec!["tenant-data", "up", "--target", "tenant-a"],
        ),
    ];

    for (operation, target, suffix) in cases {
        let expected = prefix
            .into_iter()
            .chain(suffix)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(migration_run_args(operation, &target), expected);
    }
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ryframe-xtask-migration-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FaultOperations {
    fail_install: bool,
    fail_restore: bool,
    fail_backup_cleanup: bool,
}

impl FaultOperations {
    fn has_role(path: &Path, role: &str) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(&format!(".xtask-{role}-")))
    }
}

impl FileOperations for FaultOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        if self.fail_restore && Self::has_role(source, "backup") {
            return Err(io::Error::other("注入 backup→target 恢复失败"));
        }
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        if self.fail_install && Self::has_role(source, "new") {
            return Err(io::Error::other("注入 staged→target 失败"));
        }
        fs::hard_link(source, target)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if self.fail_backup_cleanup && Self::has_role(path, "backup") {
            return Err(io::Error::other("注入 backup 清理失败"));
        }
        fs::remove_file(path)
    }
}

struct NthInstallFaults {
    hard_link_calls: AtomicU64,
    fail_at: u64,
    edit_after: Option<(u64, PathBuf)>,
}

impl FileOperations for NthInstallFaults {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        let call = self.hard_link_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_at {
            return Err(io::Error::other("注入第 N 个迁移文件安装失败"));
        }
        fs::hard_link(source, target)?;
        if self
            .edit_after
            .as_ref()
            .is_some_and(|(at, path)| *at == call && path == target)
        {
            fs::remove_file(target)?;
            fs::write(target, b"manual-after-install")?;
        }
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

#[test]
fn creates_control_migration_and_registers_it_once() {
    let root = TestRoot::new();
    root.write(
            "crates/ryframe-db/src/migration/mod.rs",
            "mod m20260820_000000_control_baseline;\nmod schema;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n",
        );
    create_migration(
        &root.0,
        MigrationScope::Control,
        "add_device",
        "20260823_010203",
    )
    .unwrap();
    let migration = fs::read_to_string(
        root.0
            .join("crates/ryframe-db/src/migration/m20260823_010203_add_device.rs"),
    )
    .unwrap();
    assert!(migration.contains("尚未实现"));
    assert!(migration.contains("追加迁移 m20260823_010203_add_device 不支持 down"));
    let registry =
        fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap();
    assert!(registry.contains("mod m20260823_010203_add_device;"));
    assert!(registry.contains("Box::new(m20260823_010203_add_device::Migration)"));
    assert!(
        create_migration(
            &root.0,
            MigrationScope::Control,
            "add_device",
            "20260823_010203",
        )
        .is_err()
    );
}

#[test]
fn tenant_migration_updates_module_and_runtime_registries() {
    let root = TestRoot::new();
    root.write(
        "crates/ryframe-tenant-db/src/migration/mod.rs",
        "mod m20260820_000000_tenant_baseline;\nmod runtime;\n",
    );
    root.write(
            "crates/ryframe-tenant-db/src/migration/runtime.rs",
            "impl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(super::m20260820_000000_tenant_baseline::Migration)]\n    }\n}\n",
        );
    create_migration(
        &root.0,
        MigrationScope::TenantData,
        "add_device",
        "20260823_010204",
    )
    .unwrap();
    let modules =
        fs::read_to_string(root.0.join("crates/ryframe-tenant-db/src/migration/mod.rs")).unwrap();
    let runtime = fs::read_to_string(
        root.0
            .join("crates/ryframe-tenant-db/src/migration/runtime.rs"),
    )
    .unwrap();
    assert!(modules.contains("mod m20260823_010204_add_device;"));
    assert!(runtime.contains("Box::new(super::m20260823_010204_add_device::Migration)"));
}

#[test]
fn invalid_migration_path_leaves_registries_unchanged() {
    let root = TestRoot::new();
    let registry = "mod m20260820_000000_control_baseline;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n";
    root.write("crates/ryframe-db/src/migration/mod.rs", registry);

    assert!(
        create_migration(
            &root.0,
            MigrationScope::Control,
            "../escape",
            "20260823_010203",
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap(),
        registry
    );
    assert!(!root.0.join("crates/ryframe-db/src/escape.rs").exists());
}

#[test]
fn migration_timestamp_must_advance_without_partial_writes() {
    let root = TestRoot::new();
    let registry = "mod m20260820_000000_control_baseline;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n";
    root.write("crates/ryframe-db/src/migration/mod.rs", registry);
    root.write(
        "crates/ryframe-db/src/migration/m20260823_020000_existing.rs",
        "existing",
    );

    assert!(
        create_migration(
            &root.0,
            MigrationScope::Control,
            "add_device",
            "20260823_010203",
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap(),
        registry
    );
    assert!(
        !root
            .0
            .join("crates/ryframe-db/src/migration/m20260823_010203_add_device.rs")
            .exists()
    );
}

#[test]
fn failed_install_restores_the_current_backup_through_shared_rollback() {
    let root = TestRoot::new();
    let target = root.0.join("registry.rs");
    fs::write(&target, b"before\n").unwrap();
    let writes = [PlannedWrite {
        path: target.clone(),
        expected: Some(b"before\n".to_vec()),
        content: b"after\n".to_vec(),
    }];
    let operations = FaultOperations {
        fail_install: true,
        fail_restore: false,
        fail_backup_cleanup: false,
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("安装失败必须进入统一回滚")
        .to_string();

    assert!(error.contains("注入 staged→target 失败"));
    assert_eq!(fs::read(&target).unwrap(), b"before\n");
    assert!(
        fs::read_dir(&root.0)
            .unwrap()
            .all(|entry| !FaultOperations::has_role(&entry.unwrap().path(), "backup"))
    );
}

#[test]
fn failed_restore_is_reported_and_keeps_the_backup_recoverable() {
    let root = TestRoot::new();
    let target = root.0.join("registry.rs");
    fs::write(&target, b"before\n").unwrap();
    let writes = [PlannedWrite {
        path: target.clone(),
        expected: Some(b"before\n".to_vec()),
        content: b"after\n".to_vec(),
    }];
    let operations = FaultOperations {
        fail_install: true,
        fail_restore: true,
        fail_backup_cleanup: false,
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("恢复失败必须与安装首错合并")
        .to_string();

    assert!(error.contains("注入 staged→target 失败"));
    assert!(error.contains("回滚未能安全完成"));
    assert!(error.contains("注入 backup→target 恢复失败"));
    assert!(error.contains("备份保留在"));
    assert!(!target.exists());
    assert!(
        fs::read_dir(&root.0)
            .unwrap()
            .any(|entry| FaultOperations::has_role(&entry.unwrap().path(), "backup"))
    );
}

#[test]
fn backup_cleanup_failure_does_not_claim_the_new_target_was_rolled_back() {
    let root = TestRoot::new();
    let target = root.0.join("registry.rs");
    fs::write(&target, b"before\n").unwrap();
    let writes = [PlannedWrite {
        path: target.clone(),
        expected: Some(b"before\n".to_vec()),
        content: b"after\n".to_vec(),
    }];
    let operations = FaultOperations {
        fail_install: false,
        fail_restore: false,
        fail_backup_cleanup: true,
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("备份清理失败必须可观测")
        .to_string();

    assert!(error.contains("迁移文件已写入，但清理备份失败"));
    assert!(error.contains("目标文件保持新内容"));
    assert_eq!(fs::read(&target).unwrap(), b"after\n");
}

#[test]
fn later_migration_install_failure_restores_every_previous_file() {
    let root = TestRoot::new();
    let paths = [
        root.0.join("migration.rs"),
        root.0.join("mod.rs"),
        root.0.join("runtime.rs"),
    ];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let writes = paths
        .iter()
        .enumerate()
        .map(|(index, path)| PlannedWrite {
            path: path.clone(),
            expected: Some(format!("before-{index}").into_bytes()),
            content: format!("after-{index}").into_bytes(),
        })
        .collect::<Vec<_>>();
    let operations = NthInstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: 2,
        edit_after: None,
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("第二个文件安装失败必须回滚")
        .to_string();

    assert!(error.contains("第 N 个迁移文件安装失败"));
    for (index, path) in paths.iter().enumerate() {
        assert_eq!(fs::read_to_string(path).unwrap(), format!("before-{index}"));
    }
    assert!(
        fs::read_dir(&root.0)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".xtask-"))
    );
}

#[test]
fn migration_rollback_preserves_manual_edit_and_blocks_retry() {
    let root = TestRoot::new();
    let paths = [
        root.0.join("migration.rs"),
        root.0.join("mod.rs"),
        root.0.join("runtime.rs"),
    ];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let writes = paths
        .iter()
        .enumerate()
        .map(|(index, path)| PlannedWrite {
            path: path.clone(),
            expected: Some(format!("before-{index}").into_bytes()),
            content: format!("after-{index}").into_bytes(),
        })
        .collect::<Vec<_>>();
    let operations = NthInstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: 2,
        edit_after: Some((1, paths[0].clone())),
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("并发编辑后续失败必须保留恢复状态")
        .to_string();

    assert!(error.contains("再次修改"));
    assert_eq!(
        fs::read_to_string(&paths[0]).unwrap(),
        "manual-after-install"
    );
    assert!(
        fs::read_dir(&root.0)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".xtask-migration-transaction-"))
    );
    let retry = FaultOperations {
        fail_install: false,
        fail_restore: false,
        fail_backup_cleanup: false,
    };
    assert!(
        commit_writes_with(&writes, &retry)
            .expect_err("恢复状态存在时必须拒绝重试")
            .to_string()
            .contains("上次迁移文件事务未完整结束")
    );
}

#[test]
fn migration_success_path_rechecks_targets_before_backup_cleanup() {
    let root = TestRoot::new();
    let paths = [root.0.join("migration.rs"), root.0.join("mod.rs")];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let writes = paths
        .iter()
        .enumerate()
        .map(|(index, path)| PlannedWrite {
            path: path.clone(),
            expected: Some(format!("before-{index}").into_bytes()),
            content: format!("after-{index}").into_bytes(),
        })
        .collect::<Vec<_>>();
    let operations = NthInstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: u64::MAX,
        edit_after: Some((1, paths[0].clone())),
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("成功路径清理备份前必须复核目标")
        .to_string();

    assert!(error.contains("安装后文件被并发修改"));
    assert_eq!(
        fs::read_to_string(&paths[0]).unwrap(),
        "manual-after-install"
    );
    assert!(
        fs::read_dir(&root.0)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| FaultOperations::has_role(&entry.path(), "backup"))
    );
}

#[test]
fn interrupted_transaction_artifacts_block_a_new_migration_write() {
    let root = TestRoot::new();
    let target = root.0.join("registry.rs");
    fs::write(&target, b"before\n").unwrap();
    let backup = root.0.join(".registry.rs.xtask-backup-999-123456-0");
    fs::write(&backup, b"recoverable-before\n").unwrap();
    let writes = [PlannedWrite {
        path: target.clone(),
        expected: Some(b"before\n".to_vec()),
        content: b"after\n".to_vec(),
    }];
    let operations = FaultOperations {
        fail_install: false,
        fail_restore: false,
        fail_backup_cleanup: false,
    };

    let error = commit_writes_with(&writes, &operations)
        .expect_err("事务遗留存在时必须安全失败")
        .to_string();

    assert!(error.contains("上次迁移文件事务未完整结束"));
    assert!(error.contains(&backup.display().to_string()));
    assert_eq!(fs::read(&target).unwrap(), b"before\n");
    assert_eq!(fs::read(&backup).unwrap(), b"recoverable-before\n");
}

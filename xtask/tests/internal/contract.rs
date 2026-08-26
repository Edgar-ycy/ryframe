use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::contract::{
    CANDIDATE_GENERATION_ARGS, ContractFileOperations, FORMAL_SYNC_ARGS, Snapshot, apply_candidate,
    apply_candidate_with_staging_hook, generated_artifact_paths, github_repository_identifier,
    install_snapshots_with, sha256_hex, validate_candidate_contract, validate_formal_sync,
    write_atomically_with,
};

const CANDIDATE_MANAGED_PATHS: &[&str] = &[
    "openapi/openapi.json",
    "src/api/generated/schema/core.ts",
    "src/api/generated/schema/system.ts",
    "src/api/generated/schema/platform.ts",
    "src/api/generated/schema/monitor.ts",
    "src/api/generated/schema/agent.ts",
    "src/api/generated/schema/index.ts",
    "src/api/generated/operations.ts",
    "src/api/generated/permissions.ts",
    "src/api/generated/menuRoutes.ts",
    "src/shared/security/passwordPolicy.generated.json",
    "src/shared/markdown/noticePolicy.generated.json",
    "src/shared/config/apiPrefix.generated.json",
    "src/api/generated/crudResources.ts",
    "src/api/generated/ownership.json",
];

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

#[test]
fn staging_contract_generation_runs_node_without_package_installation() {
    assert_eq!(
        CANDIDATE_GENERATION_ARGS,
        ["scripts/generate-api-artifacts.mjs", "--write"]
    );
    assert_eq!(
        FORMAL_SYNC_ARGS,
        [
            &["scripts/sync-api-contract.mjs"][..],
            &["scripts/generate-api-artifacts.mjs", "--write"][..],
        ]
    );
}

struct TestFrontend(PathBuf);

impl TestFrontend {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ryframe-xtask-contract-{}-{id}",
            std::process::id()
        ));
        for relative in CANDIDATE_MANAGED_PATHS {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("original:{relative}")).unwrap();
        }
        let manifest = root.join("scripts/api-artifacts.mjs");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let entries = CANDIDATE_MANAGED_PATHS[1..]
            .iter()
            .map(|path| format!("  '{path}',"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            manifest,
            format!("export const generatedArtifactPaths = Object.freeze([\n{entries}\n])\n"),
        )
        .unwrap();
        fs::write(root.join("openapi/source.json"), "formal-source").unwrap();
        let backend_openapi = root.join("backend/openapi/openapi.json");
        fs::create_dir_all(backend_openapi.parent().unwrap()).unwrap();
        fs::write(&backend_openapi, "original-backend-openapi").unwrap();
        Self(root)
    }

    fn backend(&self) -> PathBuf {
        self.0.join("backend")
    }
}

impl Drop for TestFrontend {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn candidate() -> &'static [u8] {
    b"{\n  \"info\": {\n    \"title\": \"RyFrame API\"\n  },\n  \"openapi\": \"3.1.0\"\n}\n"
}

struct AtomicFaults {
    fail_backup_cleanup: bool,
    fail_install: bool,
    fail_restore: bool,
    target: PathBuf,
}

impl ContractFileOperations for AtomicFaults {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if target == self.target
            && ((self.fail_install && name.contains(".xtask-new-"))
                || (self.fail_restore && name.contains(".xtask-backup-")))
        {
            return Err(io::Error::other("故障注入 rename"));
        }
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::hard_link(source, target)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if self.fail_backup_cleanup && name.contains(".xtask-backup-") {
            return Err(io::Error::other("故障注入 backup cleanup"));
        }
        fs::remove_file(path)
    }
}

struct InstallFaults {
    hard_link_calls: AtomicU64,
    fail_at: u64,
    edit_after: Option<(u64, PathBuf)>,
}

impl ContractFileOperations for InstallFaults {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        let call = self.hard_link_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_at {
            return Err(io::Error::other("注入第 N 个契约文件安装失败"));
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

fn backup_files(parent: &Path) -> Vec<PathBuf> {
    fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".xtask-backup-"))
        })
        .collect()
}

#[test]
fn normalizes_github_repository_identifiers() {
    assert_eq!(
        github_repository_identifier("https://github.com/Edgar-ycy/ryframe.git").unwrap(),
        "Edgar-ycy/ryframe"
    );
    assert_eq!(
        github_repository_identifier("git@github.com:Edgar-ycy/ryframe.git").unwrap(),
        "Edgar-ycy/ryframe"
    );
    assert!(github_repository_identifier("https://example.com/demo/repo").is_err());
}

#[test]
fn candidate_sync_keeps_formal_source_metadata() {
    let frontend = TestFrontend::new();
    apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| Ok(())).unwrap();
    assert_eq!(
        fs::read(frontend.0.join("openapi/openapi.json")).unwrap(),
        candidate()
    );
    assert_eq!(
        fs::read_to_string(frontend.0.join("openapi/source.json")).unwrap(),
        "formal-source"
    );
    assert_eq!(
        fs::read(frontend.backend().join("openapi/openapi.json")).unwrap(),
        candidate()
    );
    assert!(frontend.0.join("openapi/candidate.json").is_file());
}

#[test]
fn candidate_sync_rejects_changed_generation_inputs_before_install() {
    let frontend = TestFrontend::new();
    let manifest = frontend.0.join("scripts/api-artifacts.mjs");
    let result = apply_candidate_with_staging_hook(
        &frontend.backend(),
        &frontend.0,
        candidate(),
        |_| {
            fs::write(&manifest, "export const generatedArtifactPaths = []\n")?;
            Ok(())
        },
        |_| Ok(()),
    );

    let error = result
        .expect_err("生成输入变化时必须在安装前失败")
        .to_string();
    assert!(error.contains("契约生成输入在事务期间发生变化"));
    assert_eq!(
        fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
        "original:openapi/openapi.json"
    );
    assert_eq!(
        fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
        "original-backend-openapi"
    );
    assert!(!frontend.0.join("openapi/candidate.json").exists());
}

#[test]
fn multi_file_install_failure_restores_every_contract_file() {
    let frontend = TestFrontend::new();
    let paths = [
        frontend.0.join("openapi/candidate.json"),
        frontend.0.join("src/api/generated/schema/system.ts"),
        frontend.0.join("src/api/generated/menuRoutes.ts"),
    ];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let before = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("before-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let desired = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("after-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let operations = InstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: 2,
        edit_after: None,
    };

    let error = install_snapshots_with(&before, &desired, &operations)
        .expect_err("第二个文件安装失败必须回滚")
        .to_string();

    assert!(error.contains("第 N 个契约文件安装失败"));
    for (index, path) in paths.iter().enumerate() {
        assert_eq!(fs::read_to_string(path).unwrap(), format!("before-{index}"));
    }
    assert!(
        paths
            .iter()
            .flat_map(|path| fs::read_dir(path.parent().unwrap()).unwrap())
            .filter_map(std::result::Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".xtask-"))
    );
}

#[test]
fn rollback_preserves_manual_edit_after_contract_install() {
    let frontend = TestFrontend::new();
    let paths = [
        frontend.0.join("openapi/candidate.json"),
        frontend.0.join("src/api/generated/schema/system.ts"),
        frontend.0.join("src/api/generated/menuRoutes.ts"),
    ];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let before = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("before-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let desired = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("after-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let operations = InstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: 2,
        edit_after: Some((1, paths[0].clone())),
    };

    let error = install_snapshots_with(&before, &desired, &operations)
        .expect_err("并发编辑后续失败必须保留恢复状态")
        .to_string();

    assert!(error.contains("再次修改"));
    assert_eq!(
        fs::read_to_string(&paths[0]).unwrap(),
        "manual-after-install"
    );
    assert!(backup_files(paths[0].parent().unwrap()).len() == 1);
    assert!(
        fs::read_dir(paths[0].parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".xtask-contract-transaction-"))
    );
}

#[test]
fn contract_success_path_rechecks_targets_before_backup_cleanup() {
    let frontend = TestFrontend::new();
    let paths = [
        frontend.0.join("openapi/candidate.json"),
        frontend.0.join("src/api/generated/schema/system.ts"),
    ];
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("before-{index}")).unwrap();
    }
    let before = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("before-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let desired = paths
        .iter()
        .enumerate()
        .map(|(index, path)| Snapshot {
            path: path.clone(),
            content: Some(format!("after-{index}").into_bytes()),
        })
        .collect::<Vec<_>>();
    let operations = InstallFaults {
        hard_link_calls: AtomicU64::new(0),
        fail_at: u64::MAX,
        edit_after: Some((1, paths[0].clone())),
    };

    let error = install_snapshots_with(&before, &desired, &operations)
        .expect_err("成功路径清理备份前必须复核目标")
        .to_string();

    assert!(error.contains("安装后文件被并发修改"));
    assert_eq!(
        fs::read_to_string(&paths[0]).unwrap(),
        "manual-after-install"
    );
    assert!(!backup_files(paths[0].parent().unwrap()).is_empty());
}

#[test]
fn candidate_sync_rolls_back_all_managed_files_on_failure() {
    let frontend = TestFrontend::new();
    let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
    let source = frontend.0.join("openapi/source.json");
    let result = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |staging| {
        fs::write(staging.join(CANDIDATE_MANAGED_PATHS[1]), "partial")?;
        fs::write(staging.join("openapi/source.json"), "mutated")?;
        Err("模拟派生文件生成失败".into())
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
        "original:openapi/openapi.json"
    );
    assert_eq!(
        fs::read_to_string(generated).unwrap(),
        format!("original:{}", CANDIDATE_MANAGED_PATHS[1])
    );
    assert_eq!(fs::read_to_string(source).unwrap(), "formal-source");
    assert_eq!(
        fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
        "original-backend-openapi"
    );
    assert!(!frontend.0.join("openapi/candidate.json").exists());
}

#[test]
fn candidate_sync_refuses_to_overwrite_concurrent_manual_edit() {
    let frontend = TestFrontend::new();
    let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
    let result = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| {
        fs::write(&generated, "manual-edit")?;
        Ok(())
    });

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&generated).unwrap(), "manual-edit");
    assert_eq!(
        fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
        "original:openapi/openapi.json"
    );
    assert_eq!(
        fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
        "original-backend-openapi"
    );
    assert!(!frontend.0.join("openapi/candidate.json").exists());
}

#[test]
fn candidate_snapshot_precedes_staging_copy_and_preserves_interleaved_edit() {
    let frontend = TestFrontend::new();
    let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
    let result = apply_candidate_with_staging_hook(
        &frontend.backend(),
        &frontend.0,
        candidate(),
        |_| {
            fs::write(&generated, "manual-between-stage-and-install")?;
            Ok(())
        },
        |_| Ok(()),
    );

    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(&generated).unwrap(),
        "manual-between-stage-and-install"
    );
    assert_eq!(
        fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
        "original-backend-openapi"
    );
    assert!(!frontend.0.join("openapi/candidate.json").exists());
}

#[test]
fn interrupted_contract_artifacts_block_a_new_sync() {
    let frontend = TestFrontend::new();
    let target = frontend.0.join("openapi/openapi.json");
    let backup = frontend
        .0
        .join("openapi/.openapi.json.xtask-backup-999-123456-0");
    fs::write(&backup, "recoverable-openapi").unwrap();

    let error = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| Ok(()))
        .expect_err("事务遗留存在时必须安全失败")
        .to_string();

    assert!(error.contains("上次契约事务未完整结束"));
    assert!(
        error.contains(backup.file_name().unwrap().to_str().unwrap()),
        "错误应包含可恢复文件路径：{error}"
    );
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "original:openapi/openapi.json"
    );
    assert_eq!(fs::read_to_string(&backup).unwrap(), "recoverable-openapi");
}

#[test]
fn atomic_write_restores_original_when_install_fails() {
    let frontend = TestFrontend::new();
    let target = frontend.0.join("atomic.txt");
    fs::write(&target, "old").unwrap();
    let operations = AtomicFaults {
        fail_backup_cleanup: false,
        fail_install: true,
        fail_restore: false,
        target: target.clone(),
    };

    assert!(write_atomically_with(&target, b"new", &operations).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    assert!(backup_files(&frontend.0).is_empty());
}

#[test]
fn atomic_write_reports_failed_restore_and_preserves_backup() {
    let frontend = TestFrontend::new();
    let target = frontend.0.join("atomic.txt");
    fs::write(&target, "old").unwrap();
    let operations = AtomicFaults {
        fail_backup_cleanup: false,
        fail_install: true,
        fail_restore: true,
        target: target.clone(),
    };

    let error = write_atomically_with(&target, b"new", &operations)
        .expect_err("恢复失败必须显式报告")
        .to_string();
    assert!(error.contains("原文件备份保留"));
    assert!(!target.exists());
    let backups = backup_files(&frontend.0);
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "old");
}

#[test]
fn atomic_write_keeps_success_when_only_backup_cleanup_fails() {
    let frontend = TestFrontend::new();
    let target = frontend.0.join("atomic.txt");
    fs::write(&target, "old").unwrap();
    let operations = AtomicFaults {
        fail_backup_cleanup: true,
        fail_install: false,
        fail_restore: false,
        target: target.clone(),
    };

    write_atomically_with(&target, b"new", &operations).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    let backups = backup_files(&frontend.0);
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "old");
}

#[test]
fn formal_sync_requires_pinned_source_and_matching_content_hash() {
    let frontend = TestFrontend::new();
    let openapi = candidate();
    fs::write(frontend.0.join("openapi/openapi.json"), openapi).unwrap();
    let hash = sha256_hex(openapi);
    let metadata = serde_json::json!({
        "schema_version": 1,
        "backend_repository": "Edgar-ycy/ryframe",
        "backend_commit": "0123456789abcdef0123456789abcdef01234567",
        "openapi_path": "openapi/openapi.json",
        "openapi_version": "3.1.0",
        "sha256": hash,
    });
    fs::write(
        frontend.0.join("openapi/source.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let artifacts = CANDIDATE_MANAGED_PATHS[1..]
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();

    validate_formal_sync(
        &frontend.0,
        "Edgar-ycy/ryframe",
        "0123456789abcdef0123456789abcdef01234567",
        &artifacts,
    )
    .unwrap();

    let mut invalid = metadata;
    invalid["sha256"] = serde_json::Value::String("0".repeat(64));
    fs::write(
        frontend.0.join("openapi/source.json"),
        serde_json::to_vec(&invalid).unwrap(),
    )
    .unwrap();
    assert!(
        validate_formal_sync(
            &frontend.0,
            "Edgar-ycy/ryframe",
            "0123456789abcdef0123456789abcdef01234567",
            &artifacts,
        )
        .is_err()
    );
}

#[test]
fn rejects_non_ryframe_candidate() {
    assert!(validate_candidate_contract(br#"{"openapi":"2.0"}"#).is_err());
}

#[test]
fn reads_frontend_generated_artifact_manifest_and_rejects_traversal() {
    let frontend = TestFrontend::new();
    assert_eq!(
        generated_artifact_paths(&frontend.0).unwrap(),
        CANDIDATE_MANAGED_PATHS[1..]
    );
    fs::write(
        frontend.0.join("scripts/api-artifacts.mjs"),
        "export const generatedArtifactPaths = Object.freeze([\n  '../outside.ts',\n])\n",
    )
    .unwrap();
    assert!(generated_artifact_paths(&frontend.0).is_err());
}

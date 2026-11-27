use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::contract::{
    ContractFileOperations, GeneratorEntry, Snapshot, apply_candidate,
    apply_candidate_with_staging_hook, formal_sync_arguments, generated_artifact_paths,
    github_repository_identifier, install_snapshots_with, sha256_hex, validate_candidate_contract,
    validate_formal_sync, write_atomically_with,
};

const CANDIDATE_MANAGED_PATHS: &[&str] = &[
    "openapi/openapi.json",
    "src/api/generated/schema/core.ts",
    "src/api/generated/schema/system.ts",
    "src/api/generated/schema/platform.ts",
    "src/api/generated/schema/monitor.ts",
    "src/api/generated/schema/index.ts",
    "src/api/generated/operations/core.ts",
    "src/api/generated/operations/system.ts",
    "src/api/generated/operations/platform.ts",
    "src/api/generated/operations/monitor.ts",
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
    for relative in GENERATOR_ENTRIES {
        let frontend = generator_checkout();
        let root = frontend.path();
        let content = b"// frontend worktree entry\n";
        fs::write(root.join(relative), content).unwrap();
        generator_git(root, &["add", "--", relative]);
        let entry = GeneratorEntry::capture(root).unwrap();
        assert_eq!(entry.arguments(), [relative, "--write"]);
        let staging = root.join(".local-tests/contract-stage-fixture");
        fs::create_dir_all(staging.join("scripts")).unwrap();
        fs::write(staging.join(relative), content).unwrap();
        entry.verify_staging(&staging).unwrap();
        assert!(GeneratorEntry::capture(&staging).is_err());
        fs::write(staging.join(relative), b"// drift\n").unwrap();
        assert!(entry.verify_staging(&staging).is_err());

        fs::write(root.join(relative), b"// unstaged legitimate edit\n").unwrap();
        assert!(entry.verify_source(root).is_err());
        let edited = GeneratorEntry::capture(root).unwrap();
        fs::copy(root.join(relative), staging.join(relative)).unwrap();
        edited.verify_staging(&staging).unwrap();
        generator_git(root, &["add", "--", relative]);
        assert!(edited.verify_source(root).is_err());
    }
    assert_eq!(
        formal_sync_arguments("owner/backend", &"a".repeat(40), "D:/backend"),
        vec![
            "scripts/sync-api-contract.mjs".to_owned(),
            "--backend-repository".to_owned(),
            "owner/backend".to_owned(),
            "--backend-commit".to_owned(),
            "a".repeat(40),
            "--backend-worktree".to_owned(),
            "D:/backend".to_owned(),
            "--openapi-path".to_owned(),
            "openapi/openapi.json".to_owned(),
            "--write".to_owned(),
        ]
    );
}

const GENERATOR_ENTRIES: [&str; 2] = [
    "scripts/generate-api-artifacts.mjs",
    "scripts/generate-api-artifacts.ts",
];

#[test]
fn generator_rejects_missing_dual_and_unowned_entries_before_staging() {
    let frontend = generator_checkout();
    let root = frontend.path();
    let [legacy, current] = GENERATOR_ENTRIES;
    assert!(GeneratorEntry::capture(root).is_err());
    fs::write(root.join(legacy), b"// legacy\n").unwrap();
    assert!(GeneratorEntry::capture(root).is_err());
    generator_git(root, &["add", "--", legacy]);
    let entry = GeneratorEntry::capture(root).unwrap();
    fs::write(root.join(current), b"// untracked alternate\n").unwrap();
    assert!(GeneratorEntry::capture(root).is_err());
    assert!(entry.verify_source(root).is_err());
    generator_git(root, &["add", "--", current]);
    assert!(GeneratorEntry::capture(root).is_err());
    fs::remove_file(root.join(legacy)).unwrap();
    assert!(GeneratorEntry::capture(root).is_err());
    generator_git(root, &["add", "--", legacy]);
    assert_eq!(
        GeneratorEntry::capture(root).unwrap().arguments()[0],
        current
    );
    let linked = root.join("another-script");
    fs::hard_link(root.join(current), &linked).unwrap();
    assert!(GeneratorEntry::capture(root).is_err());
    fs::remove_file(linked).unwrap();
    fs::remove_file(root.join(current)).unwrap();
    assert!(GeneratorEntry::capture(root).is_err());
    assert!(!root.join(".local-tests").exists());
}

fn generator_checkout() -> tempfile::TempDir {
    let frontend = tempfile::tempdir().unwrap();
    generator_git(frontend.path(), &["init", "--quiet"]);
    fs::create_dir(frontend.path().join("scripts")).unwrap();
    frontend
}

fn generator_git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct TestFrontend(PathBuf);

impl TestFrontend {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("ryframe xtask 契约-{}-{id}", std::process::id()));
        for relative in CANDIDATE_MANAGED_PATHS {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("original:{relative}")).unwrap();
        }
        let manifest = root.join("scripts/api-artifact-manifest.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            manifest,
            serde_json::to_vec(&serde_json::json!({
                "format_version": 1, "files": &CANDIDATE_MANAGED_PATHS[1..]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("src/api/generated/ownership.json"),
            serde_json::to_vec(
                &serde_json::json!({"version": 1, "files": &CANDIDATE_MANAGED_PATHS[1..]}),
            )
            .unwrap(),
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

#[path = "contract/readonly.rs"]
mod readonly;

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
    let manifest = frontend.0.join("scripts/api-artifact-manifest.json");
    let result = apply_candidate_with_staging_hook(
        &frontend.backend(),
        &frontend.0,
        candidate(),
        |_| {
            fs::write(&manifest, br#"{"format_version":1,"files":[]}"#)?;
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

#[cfg(any(unix, windows))]
#[test]
fn candidate_rejects_linked_staging_parent_before_copying_snapshots() {
    let frontend = TestFrontend::new();
    let outside = tempfile::Builder::new()
        .prefix("contract-external-")
        .tempdir_in(frontend.0.parent().unwrap())
        .unwrap();
    let sentinel = outside.path().join("sentinel");
    fs::write(&sentinel, "保留外部原件").unwrap();
    let linked = frontend.0.join(".local-tests");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
    #[cfg(windows)]
    {
        let relative = Path::new("..").join(outside.path().file_name().unwrap());
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J", ".local-tests"])
            .arg(relative)
            .current_dir(&frontend.0)
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert_eq!(
        fs::read_to_string(linked.join("sentinel")).unwrap(),
        "保留外部原件"
    );
    let mut reached_staging = false;
    let error = apply_candidate_with_staging_hook(
        &frontend.backend(),
        &frontend.backend().join(".."),
        candidate(),
        |_| {
            reached_staging = true;
            Ok(())
        },
        |_| Ok(()),
    );
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
    let error = error.expect_err("暂存目录链接必须在复制输入前拒绝");
    assert!(error.to_string().contains("junction"), "{error}");
    assert!(!reached_staging);
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "保留外部原件");
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
    let manifest = frontend.0.join("scripts/api-artifact-manifest.json");
    for invalid in [
        serde_json::json!({"format_version":2,"files":["src/api/a.ts"]}),
        serde_json::json!({"format_version":1,"files":[]}),
        serde_json::json!({"format_version":1,"files":["src/api/a.ts","src/api/a.ts"]}),
        serde_json::json!({"format_version":1,"files":["../outside.ts"]}),
        serde_json::json!({"format_version":1,"files":["src/api/../a.ts"]}),
        serde_json::json!({"format_version":1,"files":["src//a.ts"]}),
        serde_json::json!({"format_version":1,"files":["src/api/a:stream.ts"]}),
        serde_json::json!({"format_version":1,"files":["src/api/a\\b.ts"]}),
        serde_json::json!({"format_version":1,"files":["src/api/a\nb.ts"]}),
        serde_json::json!({"format_version":1,"files":[7]}),
        serde_json::json!({"format_version":1,"files":["src/api/a.ts"],"extra":true}),
    ] {
        fs::write(&manifest, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(generated_artifact_paths(&frontend.0).is_err(), "{invalid}");
    }
    for invalid in [
        br#"{"format_version":1,"files":["src/a.ts"],"files":["src/b.ts"]}"#.as_slice(),
        br#"{"format_version":1,"format_version":1,"files":["src/a.ts"]}"#.as_slice(),
        b"not-json".as_slice(),
    ] {
        fs::write(&manifest, invalid).unwrap();
        assert!(generated_artifact_paths(&frontend.0).is_err());
    }
}

#[test]
fn candidate_sync_removes_previous_owned_artifacts_atomically() {
    let frontend = TestFrontend::new();
    let retired = "src/api/generated/operations/retired.ts";
    let old = frontend.0.join(retired);
    fs::write(&old, "old caller").unwrap();
    let manifest = frontend.0.join("src/api/generated/ownership.json");
    let mut ownership: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    ownership["files"]
        .as_array_mut()
        .unwrap()
        .push(retired.into());
    fs::write(&manifest, serde_json::to_vec(&ownership).unwrap()).unwrap();
    apply_candidate(&frontend.backend(), &frontend.0, candidate(), |staging| {
        fs::remove_file(staging.join(retired))?;
        Ok(())
    })
    .unwrap();
    assert!(!old.exists());
}

#[test]
fn candidate_sync_removes_previous_owned_shared_generated_artifact() {
    let frontend = TestFrontend::new();
    let retired = "src/shared/config/retired.generated.json";
    let old = frontend.0.join(retired);
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, "old config").unwrap();
    let manifest = frontend.0.join("src/api/generated/ownership.json");
    let mut ownership: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    ownership["files"]
        .as_array_mut()
        .unwrap()
        .push(retired.into());
    fs::write(&manifest, serde_json::to_vec(&ownership).unwrap()).unwrap();
    apply_candidate(&frontend.backend(), &frontend.0, candidate(), |staging| {
        fs::remove_file(staging.join(retired))?;
        Ok(())
    })
    .unwrap();
    assert!(!old.exists());
}

#[test]
fn candidate_sync_rejects_previous_owned_regular_source() {
    let frontend = TestFrontend::new();
    let manifest = frontend.0.join("src/api/generated/ownership.json");
    let mut ownership: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    ownership["files"]
        .as_array_mut()
        .unwrap()
        .push("src/shared/config/runtime.ts".into());
    fs::write(&manifest, serde_json::to_vec(&ownership).unwrap()).unwrap();
    let result = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| Ok(()));
    assert!(result.is_err());
}

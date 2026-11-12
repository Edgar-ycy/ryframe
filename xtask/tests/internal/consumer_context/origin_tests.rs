use super::*;
const SOURCES: [&str; 7] = [
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "xtask/Cargo.toml",
    "xtask/source-inventory.json",
    "xtask/src/frontend_origin.rs",
    "xtask/src/main.rs",
];

use crate::process::{PreservedFailure, failure_exit_code};

fn fixture() -> (tempfile::TempDir, Description) {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/consumer-origin-tests");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::tempdir_in(base).unwrap();
    let executable = root.path().join("fixture.exe");
    let mut pe = vec![0_u8; 128];
    pe[..2].copy_from_slice(b"MZ");
    pe[60..64].copy_from_slice(&64_u32.to_le_bytes());
    pe[64..68].copy_from_slice(b"PE\0\0");
    fs::write(&executable, &pe).unwrap();
    let mut sources = Vec::new();
    for relative in SOURCES {
        let path = root.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if relative == "xtask/source-inventory.json" {
            serde_json::to_vec(&serde_json::json!({"format_version":1,"files":SOURCES,"trees":["xtask/src"],"excluded_directories":[]})).unwrap()
        } else {
            relative.as_bytes().to_vec()
        };
        fs::write(&path, &bytes).unwrap();
        sources.push(Source {
            path: relative.into(),
            sha256: files::hash(&bytes),
        });
    }
    let description = Description {
        format_version: 1,
        kind: "frontend-origin-tool".into(),
        executable: Executable {
            path: executable,
            sha256: files::hash(&pe),
        },
        sources,
    };
    (root, description)
}

fn evidence(root: &Path) -> PathBuf {
    let directory = root.join(".local-tests/consumer-contexts");
    files::directory(&directory).unwrap();
    let path = directory.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&path).unwrap();
    path
}

fn prepared_fixture(root: &Path, description: &Description, evidence: &Path) -> OriginBinding {
    let held = validate_current(root, description, &description.executable.path).unwrap();
    let bytes = serde_json::to_vec(description).unwrap();
    let binding = cache(root, evidence, description, &bytes).unwrap();
    OriginBinding {
        binding,
        held,
        evidence: Some(evidence.to_owned()),
        root_identity: None,
        snapshot: None,
        source_root: root.to_owned(),
        source_paths: description
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect(),
    }
}

#[test]
fn backend_only_cache_creates_shared_lock_parent_without_consumer_preparation() {
    let (root, description) = fixture();
    let (_, directory) = super::super::origin_run(root.path()).unwrap();
    assert!(!root.path().join(".local-tests/consumer-contexts").exists());
    assert!(
        !root
            .path()
            .join(".local-tests/frontend-origin-tool.json")
            .exists()
    );
    let origin = prepared_fixture(root.path(), &description, &directory);
    origin.verify().unwrap();
    assert!(root.path().join(".local-tests/consumer-contexts").is_dir());
    assert!(
        root.path()
            .join(".local-tests/frontend-origin-tool.json")
            .is_file()
    );
    assert!(
        !root
            .path()
            .join(".local-tests/consumer-contexts/origin-binding.lock")
            .exists()
    );
    origin.finish(Ok(())).unwrap();
    assert!(!directory.join("failure.json").exists());
}

#[test]
fn describe_plan_uses_exact_hidden_arguments_and_no_environment_overrides() {
    let (root, description) = fixture();
    let command = python::command(
        root.path(),
        &description.executable.path,
        &DESCRIBE_ARGUMENTS,
    );
    assert_eq!(
        command.get_program(),
        description.executable.path.as_os_str()
    );
    assert_eq!(command.get_current_dir(), Some(root.path()));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            std::ffi::OsStr::new("__frontend-origin"),
            std::ffi::OsStr::new("describe")
        ]
    );
    assert_eq!(command.get_envs().count(), 0);
    assert!(!root.path().join(".local-tests").exists());
}

#[test]
fn suite_primary_failure_keeps_original_error_and_records_evidence() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let origin = prepared_fixture(root.path(), &description, &directory);
    let error = origin
        .finish(Err(PreservedFailure::new(
            "suite primary reason".into(),
            Some(37),
        )
        .into()))
        .unwrap_err();
    assert!(error.is::<PreservedFailure>());
    assert_eq!(error.to_string(), "suite primary reason");
    assert_eq!(failure_exit_code(error.as_ref()), Some(37));
    assert!(directory.join("failure.json").is_file());
}

#[test]
fn suite_and_source_drift_keep_primary_exit_and_both_reasons() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let mut origin = prepared_fixture(root.path(), &description, &directory);
    // 注入来源摘要不一致，独立验证双重失败的错误优先级，不绕过平台写保护。
    origin.held[1].sha256 = "0".repeat(64);
    let verification = origin.verify().unwrap_err().to_string();
    let error = origin
        .finish(Err(PreservedFailure::new(
            "suite primary reason".into(),
            Some(37),
        )
        .into()))
        .unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(37));
    assert!(error.to_string().contains("suite primary reason"));
    assert!(error.to_string().contains(&verification));
    assert!(directory.join("failure.json").is_file());
}

#[test]
fn successful_suite_cannot_hide_failed_source_verification() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let mut origin = prepared_fixture(root.path(), &description, &directory);
    origin.held[1].sha256 = "0".repeat(64);
    assert!(origin.finish(Ok(())).is_err());
    assert!(directory.join("failure.json").is_file());
}

#[test]
fn exact_sources_accept_current_bytes_and_reject_source_or_binary_drift() {
    let (root, description) = fixture();
    let held = validate_current(root.path(), &description, &description.executable.path).unwrap();
    drop(held);
    fs::write(root.path().join(SOURCES[0]), "changed").unwrap();
    assert!(validate_current(root.path(), &description, &description.executable.path).is_err());
    fs::write(&description.executable.path, "wrong binary").unwrap();
    assert!(validate_executable(&description).is_err());
}

#[test]
fn cache_reuses_exact_bytes_and_archives_valid_older_binding_before_update() {
    let (root, mut description) = fixture();
    let first = evidence(root.path());
    let bytes = serde_json::to_vec_pretty(&description).unwrap();
    let path = root.path().join(".local-tests/frontend-origin-tool.json");
    files::write_new(&path, &bytes).unwrap();
    let bound = cache(
        root.path(),
        &first,
        &description,
        &serde_json::to_vec(&description).unwrap(),
    )
    .unwrap();
    assert_eq!(bound.sha256, files::hash(&bytes));
    drop(bound);
    description.sources[0].sha256 = "b".repeat(64);
    let second = evidence(root.path());
    let updated = serde_json::to_vec(&description).unwrap();
    let bound = cache(root.path(), &second, &description, &updated).unwrap();
    assert_eq!(bound.sha256, files::hash(&updated));
    assert_eq!(
        fs::read(second.join(format!("origin-tool-{}.json", files::hash(&bytes)))).unwrap(),
        bytes
    );
}

#[test]
fn rebuilt_program_at_same_path_archives_exact_binding_and_reuses_without_writes() {
    let (root, mut description) = fixture();
    drop(validate_current(root.path(), &description, &description.executable.path).unwrap());
    let first = evidence(root.path());
    let old_bytes = serde_json::to_vec_pretty(&description).unwrap();
    let bound = cache(root.path(), &first, &description, &old_bytes).unwrap();
    let path = bound.path.clone();
    drop(bound);

    let mut rebuilt = fs::read(&description.executable.path).unwrap();
    rebuilt[127] = 1;
    let replacement = root.path().join("rebuilt.exe");
    fs::write(&replacement, &rebuilt).unwrap();
    fs::rename(&replacement, &description.executable.path).unwrap();
    description.executable.sha256 = files::hash(&rebuilt);
    drop(validate_current(root.path(), &description, &description.executable.path).unwrap());

    let second = evidence(root.path());
    let updated = serde_json::to_vec(&description).unwrap();
    let bound = cache(root.path(), &second, &description, &updated).unwrap();
    assert_eq!(bound.sha256, files::hash(&updated));
    assert_eq!(
        fs::read(second.join(format!("origin-tool-{}.json", files::hash(&old_bytes)))).unwrap(),
        old_bytes
    );
    assert_eq!(fs::read_dir(&second).unwrap().count(), 1);
    drop(bound);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let third = evidence(root.path());
    let bound = cache(root.path(), &third, &description, &updated).unwrap();
    bound.verify().unwrap();
    assert_eq!(fs::read(&path).unwrap(), updated);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read_dir(&third).unwrap().count(), 0);
}

fn assert_cache_rejected_without_overwrite(
    root: &Path,
    description: &Description,
    previous: &[u8],
    requested: &[u8],
) {
    let directory = evidence(root);
    let path = root.join(".local-tests/frontend-origin-tool.json");
    files::write_new(&path, previous).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(cache(root, &directory, description, requested).is_err());
    assert_eq!(fs::read(&path).unwrap(), previous);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
}

#[test]
fn different_old_program_missing_or_bad_digest_cannot_use_rebuild_exception() {
    for scenario in ["missing", "bad-digest", "same-file-alias"] {
        let (root, description) = fixture();
        let requested = serde_json::to_vec(&description).unwrap();
        let mut previous = parse(&requested).unwrap();
        previous.executable.path = root.path().join("other.exe");
        previous.executable.sha256 = "a".repeat(64);
        match scenario {
            "missing" => {}
            "bad-digest" => {
                fs::copy(&description.executable.path, &previous.executable.path).unwrap();
            }
            "same-file-alias" => {
                fs::hard_link(&description.executable.path, &previous.executable.path).unwrap();
            }
            _ => unreachable!(),
        }
        assert_cache_rejected_without_overwrite(
            root.path(),
            &description,
            &serde_json::to_vec(&previous).unwrap(),
            &requested,
        );
    }
}

#[test]
fn valid_old_program_at_different_path_keeps_existing_archive_behavior() {
    let (root, description) = fixture();
    let updated = serde_json::to_vec(&description).unwrap();
    let mut previous = parse(&updated).unwrap();
    previous.executable.path = root.path().join("previous.exe");
    fs::copy(&description.executable.path, &previous.executable.path).unwrap();
    let old_bytes = serde_json::to_vec_pretty(&previous).unwrap();
    let directory = evidence(root.path());
    files::write_new(
        &root.path().join(".local-tests/frontend-origin-tool.json"),
        &old_bytes,
    )
    .unwrap();
    let bound = cache(root.path(), &directory, &description, &updated).unwrap();
    assert_eq!(bound.sha256, files::hash(&updated));
    assert_eq!(
        fs::read(directory.join(format!("origin-tool-{}.json", files::hash(&old_bytes)))).unwrap(),
        old_bytes
    );
}

#[test]
fn invalid_same_path_previous_schema_or_raw_path_is_never_replaced() {
    for scenario in ["version", "source-order", "uppercase-digest", "parent-dir"] {
        let (root, description) = fixture();
        let requested = serde_json::to_vec(&description).unwrap();
        let mut previous = parse(&requested).unwrap();
        previous.executable.sha256 = "a".repeat(64);
        match scenario {
            "version" => previous.format_version = 2,
            "source-order" => previous.sources.swap(0, 1),
            "uppercase-digest" => previous.executable.sha256 = "A".repeat(64),
            "parent-dir" => {
                let directory = root.path().join("prefix");
                fs::create_dir(&directory).unwrap();
                previous.executable.path = directory.join("..").join("fixture.exe");
            }
            _ => unreachable!(),
        }
        assert_cache_rejected_without_overwrite(
            root.path(),
            &description,
            &serde_json::to_vec(&previous).unwrap(),
            &requested,
        );
    }
}

#[test]
fn unverified_new_digest_or_mismatched_request_never_updates_binding() {
    for scenario in ["bad-digest", "mismatched-request"] {
        let (root, mut description) = fixture();
        let previous = serde_json::to_vec(&description).unwrap();
        let mut requested = previous.clone();
        if scenario == "bad-digest" {
            description.executable.sha256 = "a".repeat(64);
            requested = serde_json::to_vec(&description).unwrap();
        } else {
            description.sources[0].sha256 = "b".repeat(64);
        }
        assert_cache_rejected_without_overwrite(root.path(), &description, &previous, &requested);
        assert!(
            !root
                .path()
                .join(".local-tests/consumer-contexts/origin-binding.lock")
                .exists()
        );
    }
}

#[cfg(windows)]
#[test]
fn invalid_new_pe_with_matching_digest_never_updates_binding() {
    let (root, mut description) = fixture();
    let previous = serde_json::to_vec(&description).unwrap();
    let invalid = b"SECRET_SENTINEL invalid PE";
    fs::write(&description.executable.path, invalid).unwrap();
    description.executable.sha256 = files::hash(invalid);
    let requested = serde_json::to_vec(&description).unwrap();
    assert_cache_rejected_without_overwrite(root.path(), &description, &previous, &requested);
    assert!(
        !root
            .path()
            .join(".local-tests/consumer-contexts/origin-binding.lock")
            .exists()
    );
}

#[test]
fn unknown_invalid_or_duplicate_binding_never_gets_overwritten() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let path = root.path().join(".local-tests/frontend-origin-tool.json");
    fs::write(&path, b"{\"SECRET_SENTINEL\":true}").unwrap();
    let before = fs::read(&path).unwrap();
    let error = cache(
        root.path(),
        &directory,
        &description,
        &serde_json::to_vec(&description).unwrap(),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(!error.contains("SECRET_SENTINEL"));
    assert_eq!(fs::read(&path).unwrap(), before);
    let valid = serde_json::to_string(&description).unwrap();
    let duplicate = valid.replacen('{', "{\"format_version\":1,", 1);
    assert!(parse(duplicate.as_bytes()).is_err());
    let mut invalid = serde_json::to_value(&description).unwrap();
    invalid["extra"] = serde_json::json!(true);
    assert!(parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn held_binding_detects_write_or_rejects_it_while_held() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let bytes = serde_json::to_vec(&description).unwrap();
    let bound = cache(root.path(), &directory, &description, &bytes).unwrap();
    if fs::write(&bound.path, b"changed").is_ok() {
        assert!(bound.verify().is_err());
    } else {
        bound.verify().unwrap();
    }
}

#[test]
fn replaced_same_byte_lock_is_preserved_or_replacement_is_denied() {
    let (root, _) = fixture();
    let parent = evidence(root.path());
    let path = parent.join("ownership.lock");
    let lock = CacheLock::acquire(path.clone()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let archived = parent.join("owned.lock");
    match fs::rename(&path, &archived) {
        Ok(()) => {
            fs::write(&path, &bytes).unwrap();
            assert!(lock.release().is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(archived.is_file());
        }
        Err(error) => {
            #[cfg(not(windows))]
            panic!("Unix 锁文件 rename 意外失败：{error}");
            #[cfg(windows)]
            {
                assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                assert_eq!(fs::read(&path).unwrap(), bytes);
                lock.release().unwrap();
                assert!(!path.exists());
                assert!(!archived.exists());
            }
        }
    }
}

#[test]
fn lock_parent_replacement_cannot_authorize_removal_in_foreign_directory() {
    let (root, _) = fixture();
    let parent = evidence(root.path());
    let path = parent.join("ownership.lock");
    let lock = CacheLock::acquire(path.clone()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let archived = parent.with_extension("owned");
    match fs::rename(&parent, &archived) {
        Ok(()) => {
            fs::create_dir(&parent).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert!(lock.release().is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(archived.join("ownership.lock").is_file());
        }
        Err(error) => {
            #[cfg(not(windows))]
            panic!("Unix 锁父目录 rename 意外失败：{error}");
            #[cfg(windows)]
            {
                assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                assert_eq!(fs::read(&path).unwrap(), bytes);
                lock.release().unwrap();
                assert!(!path.exists());
                assert!(!archived.exists());
            }
        }
    }
}

#[test]
fn added_source_and_omitted_worker_entry_cannot_authorize_old_binding() {
    let (root, description) = fixture();
    let directory = evidence(root.path());
    let origin = prepared_fixture(root.path(), &description, &directory);
    fs::write(root.path().join("xtask/src/new_worker.rs"), "new worker").unwrap();
    assert!(origin.verify().is_err());
    assert!(directory.join("failure.json").is_file());
    fs::remove_file(root.path().join("xtask/src/new_worker.rs")).unwrap();
    let mut omitted = description;
    omitted.sources.pop();
    assert!(validate_current(root.path(), &omitted, &omitted.executable.path).is_err());
}

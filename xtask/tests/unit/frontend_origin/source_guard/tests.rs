use super::*;
use std::{
    io::{Cursor, Write},
    time::{Duration, SystemTime},
};

struct Fixture {
    directory: tempfile::TempDir,
    sources: Vec<Source>,
}

impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/origin-source-guard-tests");
        fs::create_dir_all(&base).unwrap();
        let directory = tempfile::tempdir_in(base).unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("xtask/src")).unwrap();
        fs::write(root.join("Cargo.lock"), "lock").unwrap();
        fs::write(root.join("xtask/src/code.rs"), "private source sentinel").unwrap();
        fs::write(root.join(sources::POLICY), r#"{"format_version":1,"files":["Cargo.lock","xtask/source-inventory.json"],"trees":["xtask/src"],"excluded_directories":[]}"#).unwrap();
        let sources = sources::discover(root)
            .unwrap()
            .into_iter()
            .map(|path| Source {
                sha256: hex::encode(Sha256::digest(fs::read(root.join(&path)).unwrap())),
                path,
            })
            .collect();
        Self { directory, sources }
    }
    fn root(&self) -> &Path {
        self.directory.path()
    }
    fn request(&self) -> Request {
        Request {
            format_version: 1,
            root: self.root().to_path_buf(),
            sources: self.sources.clone(),
        }
    }
    fn check(&self) -> Result<Token> {
        verify(&self.request(), self.root(), &self.sources, |_, _| {})
    }
}

#[test]
fn token_is_stable_complete_and_never_contains_content() {
    let fixture = Fixture::new();
    let first = fixture.check().unwrap();
    assert_eq!(first, fixture.check().unwrap());
    assert_eq!(first.source_count, 3);
    let encoded = serde_json::to_value(first).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 4);
    assert!(!encoded.to_string().contains("private source sentinel"));
    assert_eq!(encoded["sha256"].as_str().unwrap().len(), 64);
}

#[test]
fn wrong_root_with_identical_file_names_and_bytes_is_rejected() {
    let fixture = Fixture::new();
    let other = Fixture::new();
    assert_eq!(fixture.sources, other.sources);
    let mut request = fixture.request();
    request.root = other.root().to_path_buf();
    assert_eq!(
        verify(&request, fixture.root(), &fixture.sources, |_, _| {})
            .unwrap_err()
            .category,
        "binding"
    );
}

#[test]
fn raw_root_dot_and_parent_aliases_are_rejected_even_for_same_physical_root() {
    let fixture = Fixture::new();
    let root = fixture.root().to_str().unwrap();
    let parent = fixture.root().parent().unwrap().to_str().unwrap();
    let leaf = fixture.root().file_name().unwrap().to_str().unwrap();
    let aliases = vec![format!("{parent}/./{leaf}"), format!("{root}/xtask/..")];
    #[cfg(windows)]
    let aliases = {
        let mut values = aliases;
        values.extend([format!("{parent}\\.\\{leaf}"), format!("{root}\\xtask\\..")]);
        values
    };
    for alias in aliases {
        assert_eq!(
            fs::canonicalize(&alias).unwrap(),
            fs::canonicalize(fixture.root()).unwrap()
        );
        let raw = serde_json::json!({"format_version":1,"root":alias,"sources":fixture.sources})
            .to_string();
        let request = parse(&mut Cursor::new(raw)).unwrap();
        assert_eq!(request.root.to_str().unwrap(), alias);
        assert_eq!(
            verify(&request, fixture.root(), &fixture.sources, |_, _| {})
                .unwrap_err()
                .category,
            "path"
        );
    }
    // 即使在 Unix 上反斜杠不是路径分隔符，协议也拒绝这类词法跳转。
    assert!(ordinary(Path::new(&format!("{root}\\.\\xtask")), true).is_err());
}

#[test]
fn ordinary_and_canonical_absolute_roots_keep_drive_or_device_prefixes_valid() {
    let fixture = Fixture::new();
    let canonical = fs::canonicalize(fixture.root()).unwrap();
    let mut request = fixture.request();
    request.root = canonical.clone();
    assert_eq!(ordinary(&request.root, true).unwrap(), canonical);
    assert!(verify(&request, fixture.root(), &fixture.sources, |_, _| {}).is_ok());
}

#[test]
fn additions_removals_and_omitted_or_changed_binding_fail_closed() {
    let fixture = Fixture::new();
    let extra = fixture.root().join("xtask/src/extra.sql");
    fs::write(&extra, "select 1").unwrap();
    assert_eq!(fixture.check().unwrap_err().category, "source_set");
    fs::remove_file(extra).unwrap();
    for changed in [false, true] {
        let mut request = fixture.request();
        if changed {
            request.sources[0].sha256 = "0".repeat(64);
        } else {
            request.sources.pop();
        }
        assert_eq!(
            verify(&request, fixture.root(), &fixture.sources, |_, _| {})
                .unwrap_err()
                .category,
            "binding"
        );
    }
    fs::remove_file(fixture.root().join("Cargo.lock")).unwrap();
    assert!(fixture.check().is_err());
}

#[test]
fn every_source_content_hash_must_match_compiled_binding() {
    for index in 0..3 {
        let fixture = Fixture::new();
        let path = fixture.root().join(&fixture.sources[index].path);
        let mut content = fs::read(&path).unwrap();
        content.push(b' ');
        fs::write(&path, content).unwrap();
        let error = fixture.check().unwrap_err();
        assert!(matches!(error.category, "digest" | "source_set"));
        assert!(!format!("{error:?}").contains("private source sentinel"));
    }
}

#[test]
fn identical_bytes_replacement_changes_token_even_with_restored_mtime() {
    let fixture = Fixture::new();
    let before = fixture.check().unwrap();
    let path = fixture.root().join("Cargo.lock");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = fixture.root().join("replacement");
    fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(replacement, path).unwrap();
    assert_ne!(before, fixture.check().unwrap());
}

#[test]
fn mtime_drift_with_unchanged_bytes_changes_aggregate() {
    let fixture = Fixture::new();
    let before = fixture.check().unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.root().join("Cargo.lock"))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(2))
        .unwrap();
    assert_ne!(before, fixture.check().unwrap());
}

#[test]
fn replacement_or_mutation_during_each_read_boundary_is_rejected() {
    for stage in ["before_open", "between_reads", "after_close"] {
        let fixture = Fixture::new();
        let selected = fs::canonicalize(fixture.root().join("Cargo.lock")).unwrap();
        let error = verify(
            &fixture.request(),
            fixture.root(),
            &fixture.sources,
            |path, point| {
                if path == selected && point == stage {
                    if stage == "between_reads" {
                        fs::write(path, "LOCK").unwrap();
                    } else {
                        let replacement = fixture.root().join("replacement");
                        fs::write(&replacement, fs::read(path).unwrap()).unwrap();
                        fs::remove_file(path).unwrap();
                        fs::rename(replacement, path).unwrap();
                    }
                }
            },
        )
        .unwrap_err();
        assert!(matches!(error.category, "identity" | "drift"));
    }
}

#[test]
fn complete_set_is_rechecked_after_last_file_read() {
    let fixture = Fixture::new();
    let error = verify(
        &fixture.request(),
        fixture.root(),
        &fixture.sources,
        |path, point| {
            if path.ends_with("code.rs") && point == "after_close" {
                fs::write(fixture.root().join("xtask/src/new.rs"), "new").unwrap();
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.category, "source_set");
}

#[test]
fn file_read_limit_retains_exact_16_mib_and_rejects_one_extra_byte() {
    let fixture = Fixture::new();
    let path = fixture.root().join("Cargo.lock");
    let mut content = vec![b'x'; FILE_LIMIT];
    fs::write(&path, &content).unwrap();
    let hash = hex::encode(Sha256::digest(&content));
    assert!(snapshot(&path, &hash, &mut |_, _| {}).is_ok());
    content.push(b'x');
    fs::write(&path, &content).unwrap();
    assert_eq!(
        snapshot(&path, &hash, &mut |_, _| {}).unwrap_err().category,
        "size"
    );
}

#[test]
fn strict_bounded_request_rejects_duplicates_unknowns_traversal_and_types() {
    let fixture = Fixture::new();
    let raw =
        serde_json::json!({"format_version":1,"root":fixture.root(),"sources":fixture.sources})
            .to_string();
    let mut boundary = raw.as_bytes().to_vec();
    boundary.resize(LIMIT, b' ');
    assert!(parse(&mut Cursor::new(&boundary)).is_ok());
    boundary.push(b' ');
    assert!(parse(&mut Cursor::new(&boundary)).is_err());
    for invalid in [
        raw.replacen('{', "{\"format_version\":1,", 1),
        raw.replace("\"format_version\":1", "\"format_version\":true"),
        raw.replace("\"format_version\":1", "\"format_version\":1.0"),
        raw.replace("Cargo.lock", "../outside"),
        raw.replace("\"root\"", "\"unknown\""),
        raw.replacen("\"sha256\":", "\"extra\":0,\"sha256\":", 1),
        "[]".into(),
        "{}".into(),
        "null".into(),
    ] {
        assert!(parse(&mut Cursor::new(invalid)).is_err());
    }
}

#[cfg(any(unix, windows))]
#[test]
fn links_in_source_parents_and_root_are_rejected_including_windows_reparse_points() {
    let fixture = Fixture::new();
    let alias = fixture.root().join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.root().join("xtask"), &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(fixture.root().join("xtask"), &alias).unwrap();
    assert!(ordinary(&alias, true).is_err());
    assert!(ordinary(&alias.join("src/code.rs"), false).is_err());
    let leaf = fixture.root().join("Cargo.lock");
    fs::remove_file(&leaf).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.root().join("xtask/src/code.rs"), &leaf).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(fixture.root().join("xtask/src/code.rs"), &leaf).unwrap();
    assert!(fixture.check().is_err());
}

#[test]
fn output_write_or_flush_failure_is_not_success() {
    struct Broken(bool);
    impl Write for Broken {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 {
                Ok(bytes.len())
            } else {
                Err(io::Error::other("private sentinel"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("private sentinel"))
        }
    }
    for flush in [true, false] {
        assert!(
            super::super::emit_to(&mut Broken(flush), &serde_json::json!({"format_version":1}))
                .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn unix_identity_retains_device_inode_and_nanosecond_ctime_mtime() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let path = fixture.root().join("Cargo.lock");
    let state = Held::open(&path, false)
        .unwrap()
        .state(&path, false)
        .unwrap();
    let info = fs::metadata(&path).unwrap();
    assert_eq!(
        (state.device, state.inode, state.size),
        (info.dev(), info.ino(), info.size())
    );
    assert_eq!(
        state.ctime,
        i128::from(info.ctime()) * 1_000_000_000 + i128::from(info.ctime_nsec())
    );
    assert_eq!(
        state.mtime,
        i128::from(info.mtime()) * 1_000_000_000 + i128::from(info.mtime_nsec())
    );
}

#[cfg(windows)]
#[test]
fn windows_identity_keeps_full_index_and_creation_time_and_rejects_all_reparse_tags() {
    use winsafe::co;
    let mut info = winsafe::BY_HANDLE_FILE_INFORMATION::default();
    info.dwFileAttributes = co::FILE_ATTRIBUTE::NORMAL;
    info.dwVolumeSerialNumber = 7;
    info.nFileIndexHigh = 9;
    info.nFileIndexLow = 11;
    info.ftCreationTime.dwHighDateTime = 13;
    info.ftCreationTime.dwLowDateTime = 15;
    let state = windows_state(&info, Path::new("fixture"), false).unwrap();
    assert_eq!(state.device, 7);
    assert_eq!(state.inode, (9_u64 << 32) | 11);
    assert_eq!(
        state.ctime,
        i128::from((13_u64 << 32) | 15) * 100 - 11_644_473_600_000_000_000_i128
    );
    // 不检查 tag 白名单：未知重解析类型也只按 REPARSE_POINT 拒绝。
    info.dwFileAttributes |= co::FILE_ATTRIBUTE::REPARSE_POINT;
    assert_eq!(
        windows_state(&info, Path::new("fixture"), false)
            .unwrap_err()
            .category,
        "link"
    );
}

#[test]
fn malformed_request_and_io_errors_never_echo_input_or_large_paths() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("private sentinel"))
        }
    }
    let error = parse(&mut Broken).err().unwrap();
    assert_eq!(error.category, "input");
    assert!(!format!("{error:?}").contains("private sentinel"));
    let error = fail("path", Path::new(&"x".repeat(LIMIT)));
    assert_eq!(error.path, "verify-sources");
}

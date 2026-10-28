use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{protocol, source_file};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = crate::workspace::root_dir()
            .join(".local-tests/source-inventory-unit")
            .join(format!("中文 来源-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet"]);
        fs::write(root.join("Cargo.toml"), b"tracked\n").unwrap();
        fs::write(root.join("deleted.rs"), b"deleted\n").unwrap();
        git(&root, &["add", "."]);
        git(
            &root,
            &[
                "-c",
                "user.name=source-unit",
                "-c",
                "user.email=source-unit@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        );
        fs::remove_file(root.join("deleted.rs")).unwrap();
        fs::write(root.join("Cargo.toml"), b"modified\n").unwrap();
        fs::write(root.join("新增 文件.txt"), b"first").unwrap();
        Self(root)
    }

    fn observe(&self, action: &str) -> Value {
        protocol::request(&json!({"action":action,"root":self.0})).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let result = command.env("GIT_OPTIONAL_LOCKS", "0").output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn git_observation_binds_raw_patch_untracked_bytes_index_modes_and_deleted_files() {
    let fixture = Fixture::new();
    let root = &fixture.0;
    let value = fixture.observe("snapshot");
    let patch = git(root, &["diff", "--binary", "HEAD"]);
    assert_eq!(value["patch"], json!(patch));
    assert_eq!(
        value["snapshot"]["patch_sha256"],
        hex::encode(Sha256::digest(&patch))
    );
    assert_eq!(value["snapshot"]["files"][0]["path"], "新增 文件.txt");
    assert_eq!(
        value["snapshot"]["files"][0]["sha256"],
        hex::encode(Sha256::digest(b"first"))
    );
    let snapshot = fixture.observe("source-snapshot");
    assert_eq!(snapshot["clean"], false);
    let head = snapshot["head"].as_str().unwrap();
    let fingerprint =
        protocol::request(&json!({"action":"worktree-fingerprint","root":root,"commit":head}))
            .unwrap();
    let mut digest = Sha256::new();
    let diff = git(
        root,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
    );
    for bytes in [
        head.as_bytes(),
        diff.as_slice(),
        "新增 文件.txt".as_bytes(),
        b"first",
    ] {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    assert_eq!(
        fingerprint,
        format!("sha256:{}", hex::encode(digest.finalize()))
    );
    let inventory =
        protocol::request(&json!({"action":"capture","root":root,"source":snapshot})).unwrap();
    assert_eq!(inventory["files"].as_array().unwrap().len(), 2);
    assert_eq!(inventory["files"][0]["path"], "Cargo.toml");
    assert_eq!(inventory["files"][1]["path"], "新增 文件.txt");
    assert_eq!(
        inventory["guard"]["index_sha256"],
        hex::encode(Sha256::digest(git(root, &["ls-files", "--stage", "-z"])))
    );
    assert_eq!(inventory["source"]["worktree_fingerprint"], fingerprint);
    let guard = fixture.observe("file-inventory");
    assert_eq!(guard["modes_sha256"], inventory["guard"]["modes_sha256"]);
    let path = root.join("新增 文件.txt");
    let metadata = fs::metadata(&path).unwrap();
    fs::write(&path, b"other").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(metadata.modified().unwrap())
                .set_accessed(metadata.accessed().unwrap()),
        )
        .unwrap();
    assert_ne!(
        protocol::request(&json!({"action":"worktree-fingerprint","root":root,"commit":head}))
            .unwrap(),
        fingerprint
    );
    assert!(protocol::request(&json!({"action":"capture","root":root,"source":snapshot})).is_err());
}

#[test]
fn observation_rejects_non_root_invalid_head_and_non_files() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("nested")).unwrap();
    let actions = [
        "snapshot",
        "source-snapshot",
        "file-inventory",
        "capture",
        "worktree-fingerprint",
    ];
    let request = |action: &str, root: &Path| {
        let mut args = json!({"action":action,"root":root});
        if action == "capture" {
            args["source"] = Value::Null;
        }
        if action == "worktree-fingerprint" {
            args["commit"] = json!("a".repeat(40));
        }
        args
    };
    for action in actions {
        assert!(
            protocol::request(&request(action, &fixture.0.join("nested"))).is_err(),
            "子目录：{action}"
        );
    }
    assert!(source_file(&fixture.0, "nested", false).is_err());
    assert!(source_file(&fixture.0, "../outside", true).is_err());
    assert!(source_file(&fixture.0, "missing", true).unwrap().is_none());
    fs::write(fixture.0.join(".git/HEAD"), b"invalid\n").unwrap();
    // 每个 Git 采集入口都必须拒绝向上发现的父仓库，包含原始 snapshot 入口。
    for action in actions {
        assert!(
            protocol::request(&request(action, &fixture.0)).is_err(),
            "损坏 HEAD：{action}"
        );
    }
}

#[cfg(unix)]
#[test]
fn intermediate_and_leaf_symlinks_are_rejected_before_reading() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(&fixture.0, fixture.0.join("linked")).unwrap();
    std::os::unix::fs::symlink(fixture.0.join("Cargo.toml"), fixture.0.join("leaf")).unwrap();
    for path in ["linked/Cargo.toml", "leaf"] {
        assert!(source_file(&fixture.0, path, false).is_err());
    }
    assert!(super::super::file_inventory(&fixture.0).is_err());
    assert!(
        protocol::request(&json!({"action":"capture","root":fixture.0,"source":null})).is_err()
    );
}

#[cfg(windows)]
#[test]
fn intermediate_junction_is_rejected_before_file_read() {
    let fixture = Fixture::new();
    let junction = fixture.0.join("linked");
    let result = Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&fixture.0)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let observed = source_file(&fixture.0, "linked/Cargo.toml", false);
    fs::create_dir(fixture.0.join("xtask")).unwrap();
    fs::write(
        fixture.0.join(crate::frontend_origin::sources::POLICY),
        br#"{"format_version":1,"files":["Cargo.toml","linked/Cargo.toml"],"trees":[],"excluded_directories":[]}"#,
    ).unwrap();
    let manifest = crate::frontend_origin::manifest::capture(&fixture.0);
    fs::remove_dir(&junction).unwrap();
    assert!(observed.is_err());
    assert!(manifest.is_err());
}

#[cfg(unix)]
#[test]
fn executable_mode_changes_are_bound_even_when_content_is_identical() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let path = fixture.0.join("Cargo.toml");
    let before = fixture.observe("file-inventory");
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode ^ 0o111)).unwrap();
    let after = fixture.observe("file-inventory");
    assert_eq!(before["files"], after["files"]);
    assert_ne!(before["modes_sha256"], after["modes_sha256"]);
}

#[test]
fn git_environment_override_child_observes_only_explicit_repository() {
    let fixture = Fixture::new();
    let snapshot = fixture.observe("source-snapshot");
    assert_eq!(
        snapshot["head"].as_str().unwrap(),
        String::from_utf8(git(&fixture.0, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
    );
    assert_eq!(snapshot["files"][0]["path"], "新增 文件.txt");
}

#[test]
fn inherited_git_repository_index_and_config_overrides_cannot_change_source() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact",
            "recovery::clone::native::source::tests::git::git_environment_override_child_observes_only_explicit_repository",
            "--nocapture"])
        .env("GIT_DIR", "not-a-repository")
        .env("GIT_WORK_TREE", "not-a-worktree")
        .env("GIT_INDEX_FILE", "not-an-index")
        .env("GIT_CONFIG_COUNT", "invalid")
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

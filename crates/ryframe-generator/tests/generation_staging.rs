use ryframe_generator::ResourceError;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[path = "../src/resource/writer/transaction.rs"]
pub(crate) mod transaction;
mod resource {
    pub(crate) use crate::transaction as write_transaction;
}

#[path = "../src/command/staging/install.rs"]
mod install;

#[path = "../src/command/staging/manifest.rs"]
mod manifest;
#[path = "../src/command/staging/snapshot.rs"]
mod snapshot;

fn fixture_root() -> tempfile::TempDir {
    let parent =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/generation-staging");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, bytes).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git 夹具失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn archive_candidate_preserves_binary_and_unicode_inputs_without_touching_source() {
    let root = fixture_root();
    let source = root.path().join("源码 有空格");
    write(
        &source,
        "crates/business/src/lib.rs",
        b"pub fn source() {}\n",
    );
    write(&source, "images/二进制.bin", &[0, 1, 255, 128]);
    write(&source, "target/cache", b"cache");
    write(&source, ".local-tests/private", b"private");
    write(&source, "node_modules/dependency", b"dependency");
    let before = snapshot::SourceSnapshot::capture(&source).unwrap();
    assert_eq!(before.root, fs::canonicalize(&source).unwrap());
    let candidate = root.path().join("候选 有空格");
    before.copy_to(&candidate).unwrap();
    assert_eq!(
        fs::read(candidate.join("images/二进制.bin")).unwrap(),
        [0, 1, 255, 128]
    );
    assert!(!candidate.join("target").exists());
    assert!(!candidate.join(".local-tests").exists());
    assert!(!candidate.join("node_modules").exists());
    assert!(before.changes(&candidate).unwrap().is_empty());
    write(
        &candidate,
        "crates/business/src/lib.rs",
        b"pub fn generated() {}\n",
    );
    write(
        &candidate,
        "crates/business/src/新增.rs",
        "// 新文件\n".as_bytes(),
    );
    write(&candidate, "openapi/candidate.json", b"{}\n");
    fs::remove_file(candidate.join("images/二进制.bin")).unwrap();
    let changes = before.changes(&candidate).unwrap();
    assert_eq!(changes.len(), 4);
    let deleted = changes
        .iter()
        .find(|change| change.relative == Path::new("images/二进制.bin"))
        .unwrap();
    assert_eq!(deleted.before.as_deref(), Some([0, 1, 255, 128].as_slice()));
    assert!(deleted.after.is_none());
    before.verify().unwrap();
    assert_eq!(
        fs::read(source.join("crates/business/src/lib.rs")).unwrap(),
        b"pub fn source() {}\n"
    );
    assert!(before.copy_to(&candidate).unwrap_err().contains("不会覆盖"));
}

#[test]
fn concurrent_source_create_edit_delete_and_candidate_marker_are_all_detected() {
    for operation in ["create", "edit", "delete", "marker"] {
        let root = fixture_root();
        write(root.path(), "src/lib.rs", b"original");
        let before = snapshot::SourceSnapshot::capture(root.path()).unwrap();
        match operation {
            "create" => write(root.path(), "src/new.rs", b"new"),
            "edit" => write(root.path(), "src/lib.rs", b"modified"),
            "delete" => fs::remove_file(root.path().join("src/lib.rs")).unwrap(),
            "marker" => write(root.path(), "openapi/candidate.json", b"{}"),
            _ => unreachable!(),
        }
        assert!(
            before.verify().unwrap_err().contains("本批未安装"),
            "{operation}"
        );
    }
}

#[test]
fn git_snapshot_keeps_untracked_and_tracked_ignored_inputs_but_omits_private_files() {
    let root = fixture_root();
    git(root.path(), &["init", "--quiet"]);
    write(
        root.path(),
        ".gitignore",
        b"private/\n.env.*\nopenapi/candidate.json\n",
    );
    write(root.path(), "src/lib.rs", b"tracked");
    write(root.path(), "src/deleted.rs", b"deleted");
    write(root.path(), ".env.production", b"VITE_PUBLIC_VALUE=test");
    git(root.path(), &["add", "--", ".gitignore", "src"]);
    git(root.path(), &["add", "--force", "--", ".env.production"]);
    fs::remove_file(root.path().join("src/deleted.rs")).unwrap();
    write(root.path(), "src/new.rs", b"untracked");
    write(root.path(), "private/secret", b"private");
    write(root.path(), "openapi/candidate.json", b"candidate");
    let before = snapshot::SourceSnapshot::capture(root.path()).unwrap();
    let candidate_parent = fixture_root();
    let candidate = candidate_parent.path().join("candidate");
    before.copy_to(&candidate).unwrap();
    assert!(!candidate.join(".git").exists());
    assert!(!candidate.join("private").exists());
    assert!(!candidate.join("src/deleted.rs").exists());
    assert_eq!(
        fs::read(candidate.join(".env.production")).unwrap(),
        b"VITE_PUBLIC_VALUE=test"
    );
    assert_eq!(
        fs::read(candidate.join("src/new.rs")).unwrap(),
        b"untracked"
    );
    assert_eq!(
        fs::read(candidate.join("openapi/candidate.json")).unwrap(),
        b"candidate"
    );
    assert!(before.changes(&candidate).unwrap().is_empty());
    before.verify().unwrap();
}

#[test]
fn source_paths_cannot_escape_or_use_absolute_targets() {
    let root = fixture_root();
    for invalid in [Path::new("../outside"), Path::new(""), root.path()] {
        assert!(snapshot::safe_path(root.path(), invalid).is_err());
    }
}

#[test]
fn one_journal_installs_both_repositories_and_keeps_untouched_files() {
    let root = fixture_root();
    let sources = [root.path().join("backend"), root.path().join("frontend")];
    for source in &sources {
        write(source, "src/old.rs", b"original");
        write(source, "src/remove.rs", b"remove");
        write(source, "src/keep.rs", b"keep");
    }
    let snapshots = sources
        .iter()
        .map(|root| snapshot::SourceSnapshot::capture(root).unwrap())
        .collect::<Vec<_>>();
    let mut stages = Vec::new();
    for (index, source) in snapshots.iter().enumerate() {
        let candidate = root.path().join(format!("candidate-{index}"));
        source.copy_to(&candidate).unwrap();
        write(&candidate, "src/old.rs", b"generated");
        write(&candidate, "src/new.rs", b"new");
        fs::remove_file(candidate.join("src/remove.rs")).unwrap();
        stages.push(install::prepare(source, source.changes(&candidate).unwrap()).unwrap());
    }
    install::commit(stages).unwrap();
    for source in &sources {
        assert_eq!(fs::read(source.join("src/old.rs")).unwrap(), b"generated");
        assert_eq!(fs::read(source.join("src/new.rs")).unwrap(), b"new");
        assert_eq!(fs::read(source.join("src/keep.rs")).unwrap(), b"keep");
        assert!(!source.join("src/remove.rs").exists());
    }
}

#[test]
fn missing_frontend_staged_output_rolls_back_already_installed_backend() {
    let root = fixture_root();
    let sources = [root.path().join("backend"), root.path().join("frontend")];
    for source in &sources {
        write(source, "src/old.rs", b"original");
    }
    let snapshots = sources
        .iter()
        .map(|root| snapshot::SourceSnapshot::capture(root).unwrap())
        .collect::<Vec<_>>();
    let mut stages = Vec::new();
    for (index, source) in snapshots.iter().enumerate() {
        let candidate = root.path().join(format!("candidate-{index}"));
        source.copy_to(&candidate).unwrap();
        write(&candidate, "src/old.rs", b"generated");
        stages.push(install::prepare(source, source.changes(&candidate).unwrap()).unwrap());
    }
    fs::remove_file(stages[1].directory.path().join("src/old.rs")).unwrap();
    let error = install::commit(stages).unwrap_err();
    assert!(error.contains("本批已回滚"));
    for source in &sources {
        assert_eq!(fs::read(source.join("src/old.rs")).unwrap(), b"original");
    }
    for snapshot in snapshots {
        snapshot.verify().unwrap();
    }
}

#[test]
fn source_changes_after_staging_refuse_all_installations() {
    let root = fixture_root();
    let source = root.path().join("source");
    write(&source, "src/lib.rs", b"original");
    let snapshot = snapshot::SourceSnapshot::capture(&source).unwrap();
    let candidate = root.path().join("candidate");
    snapshot.copy_to(&candidate).unwrap();
    write(&candidate, "src/lib.rs", b"generated");
    let stage = install::prepare(&snapshot, snapshot.changes(&candidate).unwrap()).unwrap();
    write(&source, "src/lib.rs", b"concurrent");
    assert!(
        install::commit(vec![stage])
            .unwrap_err()
            .contains("本批未安装")
    );
    assert_eq!(fs::read(source.join("src/lib.rs")).unwrap(), b"concurrent");
}

#[test]
fn tampered_staged_content_is_rejected_and_the_original_file_is_restored() {
    let root = fixture_root();
    let source = root.path().join("source");
    write(&source, "src/lib.rs", b"original");
    let snapshot = snapshot::SourceSnapshot::capture(&source).unwrap();
    let candidate = root.path().join("candidate");
    snapshot.copy_to(&candidate).unwrap();
    write(&candidate, "src/lib.rs", b"generated");
    let stage = install::prepare(&snapshot, snapshot.changes(&candidate).unwrap()).unwrap();
    write(stage.directory.path(), "src/lib.rs", b"tampered");
    let error = install::commit(vec![stage]).unwrap_err();
    assert!(error.contains("本批已回滚") && error.contains("compare-and-swap"));
    assert_eq!(fs::read(source.join("src/lib.rs")).unwrap(), b"original");
    snapshot.verify().unwrap();
}

#[test]
fn overlapping_roots_reserved_paths_and_binary_outputs_fail_before_installation() {
    let root = fixture_root();
    write(root.path(), "nested/file.rs", b"source");
    let parent = snapshot::SourceSnapshot::capture(root.path()).unwrap();
    let child = snapshot::SourceSnapshot::capture(&root.path().join("nested")).unwrap();
    let stages = vec![
        install::prepare(&parent, vec![]).unwrap(),
        install::prepare(&child, vec![]).unwrap(),
    ];
    assert!(install::commit(stages).unwrap_err().contains("不能重叠"));
    for (path, bytes) in [
        (".backup/file", b"reserved".as_slice()),
        ("src/binary", [255].as_slice()),
        ("../outside", b"outside".as_slice()),
    ] {
        assert!(
            install::prepare(
                &parent,
                vec![snapshot::FileChange {
                    relative: path.into(),
                    before: None,
                    after: Some(bytes.to_vec())
                }]
            )
            .is_err()
        );
        assert!(!root.path().join(path).exists());
    }
    parent.verify().unwrap();
}

#[test]
fn candidate_cargo_paths_keep_internal_copies_and_verify_external_workspaces() {
    let root = fixture_root();
    let source = root.path().join("source");
    let external = root.path().join("external");
    let candidate = root.path().join("candidate");
    write(
        &external,
        "Cargo.toml",
        b"[workspace]\nmembers=['crates/lib']\n",
    );
    write(
        &external,
        "crates/lib/Cargo.toml",
        b"[package]\nname='external'\nversion='0.1.0'\n",
    );
    write(
        &external,
        "crates/lib/src/lib.rs",
        b"pub fn external() {}\n",
    );
    write(
        &source,
        "crates/internal/Cargo.toml",
        b"[package]\nname='internal'\nversion='0.1.0'\n",
    );
    let internal = source.join("crates/internal");
    let manifest_text = format!(
        "[workspace]\nmembers=['crates/internal']\n[workspace.dependencies]\nexternal={{path='../external/crates/lib'}}\ninternal={{path='crates/internal'}}\nabsolute={{path={:?}}}\n[patch.crates-io]\npatched={{path='../external/crates/lib'}}\n[package.metadata.example]\npath='../metadata-is-not-a-dependency'\n",
        internal.to_string_lossy()
    );
    write(&source, "Cargo.toml", manifest_text.as_bytes());
    write(
        &source,
        "crates/internal/src/lib.rs",
        b"pub fn internal() {}\n",
    );
    let before = snapshot::SourceSnapshot::capture(&source).unwrap();
    before.copy_to(&candidate).unwrap();
    let external_inputs = manifest::normalize_external_paths(&before, &candidate).unwrap();
    assert!(!external_inputs.is_empty());
    let normalized: toml::Value =
        toml::from_str(&fs::read_to_string(candidate.join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(
        normalized["workspace"]["dependencies"]["internal"]["path"].as_str(),
        Some("crates/internal")
    );
    assert_eq!(
        fs::canonicalize(
            normalized["workspace"]["dependencies"]["external"]["path"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        fs::canonicalize(external.join("crates/lib")).unwrap()
    );
    assert_eq!(
        fs::canonicalize(
            normalized["workspace"]["dependencies"]["absolute"]["path"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        fs::canonicalize(candidate.join("crates/internal")).unwrap()
    );
    assert_eq!(
        normalized["package"]["metadata"]["example"]["path"].as_str(),
        Some("../metadata-is-not-a-dependency")
    );
    assert_eq!(
        fs::read_to_string(source.join("Cargo.toml")).unwrap(),
        manifest_text
    );
    before.verify().unwrap();
    let candidate_before = snapshot::SourceSnapshot::capture(&candidate).unwrap();
    write(
        &candidate,
        "crates/internal/src/generated.rs",
        b"// generated\n",
    );
    let changes = candidate_before.changes(&candidate).unwrap();
    assert_eq!(changes.len(), 1, "临时 Cargo 路径展开不能成为交付差异");
    for external in &external_inputs {
        external.verify().unwrap();
    }
    write(&external, "Cargo.toml", b"[workspace]\nmembers=[]\n");
    assert!(
        external_inputs
            .iter()
            .any(|external| external.verify().is_err()),
        "外部 workspace 继承配置也必须核对"
    );
}

#[test]
fn candidate_path_normalization_covers_target_dev_build_dependencies_and_leaves_unused_paths() {
    let root = fixture_root();
    let source = root.path().join("source");
    let external = root.path().join("external");
    write(
        &external,
        "Cargo.toml",
        b"[package]\nname='external'\nversion='0.1.0'\n[workspace]\n",
    );
    let text = "[package]\nname='source'\nversion='0.1.0'\n[workspace]\n[dependencies]\nexternal={path='../external'}\n[dev-dependencies]\nexternal={path='../external'}\nunused={path='../missing-dev-helper'}\n[build-dependencies]\nexternal={path='../external'}\n[target.'cfg(windows)'.dependencies]\nexternal={path='../external'}\n";
    write(&source, "Cargo.toml", text.as_bytes());
    let before = snapshot::SourceSnapshot::capture(&source).unwrap();
    let candidate = root.path().join("candidate");
    before.copy_to(&candidate).unwrap();
    manifest::normalize_external_paths(&before, &candidate).unwrap();
    let normalized: toml::Value =
        toml::from_str(&fs::read_to_string(candidate.join("Cargo.toml")).unwrap()).unwrap();
    for dependencies in [
        &normalized["dependencies"],
        &normalized["dev-dependencies"],
        &normalized["build-dependencies"],
        &normalized["target"]["cfg(windows)"]["dependencies"],
    ] {
        assert_eq!(
            fs::canonicalize(dependencies["external"]["path"].as_str().unwrap()).unwrap(),
            fs::canonicalize(&external).unwrap()
        );
    }
    assert_eq!(
        normalized["dev-dependencies"]["unused"]["path"].as_str(),
        Some("../missing-dev-helper")
    );
    before.verify().unwrap();
}

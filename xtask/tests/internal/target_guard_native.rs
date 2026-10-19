use std::{fs, path::PathBuf};

use crate::{
    recovery::clone::native::target_verify::{guard, tree},
    workspace,
};

struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    target: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let parent = backend.join(".local-tests/target-generation-guard");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        Self {
            backend,
            _directory: directory,
            target,
        }
    }
}

#[test]
fn generation_holds_shared_copy_guard_and_releases_only_its_directory_lock() {
    let fixture = Fixture::new();
    let generation = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    generation.check().unwrap();
    let tree_before = tree::snapshot(&fixture.backend, &fixture.target, true).unwrap();
    assert!(fixture.target.join("initialize.lock").is_dir());
    assert_eq!(
        fs::metadata(fixture.target.join("copy-target.guard"))
            .unwrap()
            .len(),
        1
    );
    assert!(guard::Generation::acquire(&fixture.backend, &fixture.target).is_err());
    generation.finish(Ok(())).unwrap();
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap(),
        tree_before
    );
    assert_eq!(
        fs::read(fixture.target.join("copy-target.guard")).unwrap(),
        [0]
    );
    assert!(!fixture.target.join("initialize.lock").exists());
    assert!(fixture.target.join("copy-target.guard").is_file());
    let generation = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    let error = generation
        .finish::<()>(Err("已确认的测试失败".into()))
        .unwrap_err();
    assert!(error.to_string().contains("已确认的测试失败"));
    assert!(!fixture.target.join("initialize.lock").exists());
}

#[test]
fn generation_refuses_residual_lock_without_replacing_evidence() {
    let fixture = Fixture::new();
    let lock = fixture.target.join("initialize.lock");
    fs::create_dir(&lock).unwrap();
    fs::write(lock.join("unknown.json"), b"unknown existing operation").unwrap();
    assert!(guard::Generation::acquire(&fixture.backend, &fixture.target).is_err());
    assert_eq!(
        fs::read(lock.join("unknown.json")).unwrap(),
        b"unknown existing operation"
    );
    assert!(!fixture.target.join("copy-target.guard").exists());
}

#[test]
fn release_failure_keeps_unknown_files_and_preserves_the_primary_error() {
    let fixture = Fixture::new();
    let generation = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    let evidence = fixture.target.join("initialize.lock/unknown.json");
    fs::write(&evidence, b"retain for reconciliation").unwrap();
    let error = generation
        .finish::<()>(Err("primary fixture failure".into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("primary fixture failure"));
    assert!(error.contains("目标锁释放同时失败"));
    assert_eq!(fs::read(evidence).unwrap(), b"retain for reconciliation");
    assert!(fixture.target.join("initialize.lock").is_dir());
}

#[test]
fn target_tree_matches_python_for_unicode_files_and_empty_directories() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.target.join("空目录")).unwrap();
    fs::create_dir(fixture.target.join("中文 空格")).unwrap();
    fs::write(fixture.target.join("中文 空格/receipt.json"), b"{}\n").unwrap();
    let actual = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let output = std::process::Command::new(std::env::var_os("RYFRAME_PYTHON").unwrap_or_else(|| "python".into()))
        .args(["-X", "utf8", "-B", "-c", "import json,sys; from pathlib import Path; from devex_clone_target_binding import target_files; print(json.dumps(target_files(Path(sys.argv[1]))))"])
        .arg(&fixture.target).env("PYTHONPATH", fixture.backend.join("tools/python"))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        actual,
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    );
    let files = tree::files(&fixture.target, &actual).unwrap();
    assert_eq!(files, [fixture.target.join("中文 空格/receipt.json")]);
    let protected = guard::Inputs::open(files).unwrap();
    protected.check().unwrap();
}

#[test]
fn target_tree_refuses_extra_hard_links_and_unknown_lock_children() {
    let fixture = Fixture::new();
    let path = fixture.target.join("receipt.json");
    fs::write(&path, b"{}\n").unwrap();
    fs::hard_link(&path, fixture.target.join("alias.json")).unwrap();
    assert!(tree::snapshot(&fixture.backend, &fixture.target, false).is_err());
    fs::remove_file(fixture.target.join("alias.json")).unwrap();
    let generation = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    fs::write(
        fixture.target.join("initialize.lock/unknown.json"),
        b"unknown",
    )
    .unwrap();
    assert!(tree::snapshot(&fixture.backend, &fixture.target, true).is_err());
    assert!(generation.finish(Ok(())).is_err());
}

#[test]
fn protected_inputs_keep_the_opened_file_identity_and_content() {
    let fixture = Fixture::new();
    let path = fixture.target.join("request.json");
    fs::write(&path, b"original").unwrap();
    let inputs = guard::Inputs::open([path.clone()]).unwrap();
    inputs.check().unwrap();
    #[cfg(windows)]
    {
        assert!(fs::write(&path, b"changed!").is_err());
        assert!(fs::remove_file(&path).is_err());
        inputs.check().unwrap();
    }
    #[cfg(unix)]
    {
        fs::write(&path, b"changed!").unwrap();
        assert!(inputs.check().is_err());
    }
    drop(inputs);
    fs::write(path, b"after release").unwrap();
}

#[test]
fn protected_inputs_bind_existing_hard_links_and_release_partial_handles() {
    let fixture = Fixture::new();
    let path = fixture.target.join("bound.json");
    let alias = fixture.target.join("alias.json");
    fs::write(&path, b"fixed receipt").unwrap();
    fs::hard_link(&path, &alias).unwrap();
    let inputs = guard::Inputs::open([path.clone()]).unwrap();
    inputs.check().unwrap();
    #[cfg(windows)]
    assert!(fs::write(&alias, b"write through alias").is_err());
    #[cfg(unix)]
    {
        fs::write(&alias, b"write through alias").unwrap();
        assert!(inputs.check().is_err());
    }
    drop(inputs);
    fs::remove_file(alias).unwrap();
    let missing = fixture.target.join("missing.json");
    assert!(guard::Inputs::open([path.clone(), missing]).is_err());
    fs::write(path, b"handle released").unwrap();
}

#[test]
fn generation_detects_replaced_directory_and_does_not_remove_a_foreign_lock() {
    let fixture = Fixture::new();
    let generation = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    let lock = fixture.target.join("initialize.lock");
    let moved = fixture.target.join("moved-lock");
    #[cfg(windows)]
    {
        assert!(fs::rename(&lock, &moved).is_err());
        generation.finish(Ok(())).unwrap();
    }
    #[cfg(unix)]
    {
        fs::rename(&lock, &moved).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(generation.check().is_err());
        assert!(generation.finish(Ok(())).is_err());
        assert!(lock.is_dir());
        assert!(moved.is_dir());
    }
}

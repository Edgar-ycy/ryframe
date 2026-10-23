use super::*;
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}
fn repository() -> tempfile::TempDir {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temp = tempfile::tempdir_in(local).unwrap();
    git(temp.path(), &["init", "-q"]);
    git(
        temp.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(temp.path(), &["config", "user.name", "隔离夹具"]);
    fs::write(temp.path().join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
    fs::write(temp.path().join("package.json"), "{}").unwrap();
    fs::write(temp.path().join(".gitignore"), ".local-tests/\n").unwrap();
    git(temp.path(), &["add", "."]);
    git(temp.path(), &["commit", "-qm", "fixture"]);
    fs::create_dir(temp.path().join(".local-tests")).unwrap();
    temp
}

#[test]
fn formal_snapshot_rejects_dirty_wrong_head_and_untracked_files() {
    let repo = repository();
    let captured = snapshot::capture(repo.path()).unwrap();
    let head = captured.receipt["head"].as_str().unwrap();
    snapshot::formal(&captured, head).unwrap();
    assert!(snapshot::formal(&captured, &"a".repeat(40)).is_err());
    fs::write(repo.path().join("new.txt"), "来源文件").unwrap();
    let changed = snapshot::capture(repo.path()).unwrap();
    assert_ne!(captured.receipt, changed.receipt);
    assert!(snapshot::formal(&changed, head).is_err());
    fs::write(
        repo.path().join("Cargo.toml"),
        "[workspace]\nmembers=[]\n#changed\n",
    )
    .unwrap();
    assert!(!snapshot::capture(repo.path()).unwrap().patch.is_empty());
}

#[test]
fn validates_independent_roots_new_ignored_output_and_exact_git_root() {
    let backend = repository();
    let frontend = repository();
    let options = FixturePrepareOptions {
        output_dir: backend.path().join(".local-tests/business 中文"),
        expected_sources: None,
    };
    validate(&options, backend.path(), frontend.path()).unwrap();
    assert!(validate(&options, backend.path(), backend.path()).is_err());
    fs::create_dir(&options.output_dir).unwrap();
    assert!(validate(&options, backend.path(), frontend.path()).is_err());
}

#[test]
fn business_commands_keep_registration_candidate_contract_and_offline_lock_refresh() {
    let values = commands(Path::new("backend"), Path::new("frontend"));
    let commands = values.iter().map(|(_, v)| v.join(" ")).collect::<Vec<_>>();
    assert!(commands[0].contains("new-business order --write"));
    assert!(commands[2].contains("generate-lockfile --offline"));
    assert!(commands[4].contains("pnpm install --offline --frozen-lockfile"));
    assert!(commands[6].contains("xtask data migrate reconcile --write"));
    assert!(commands[7].contains("xtask generate api --write --frontend-dir frontend"));
    assert!(
        !commands
            .join(" ")
            .contains("RYFRAME_GENERATOR_FRONTEND_DIR")
    );
}

struct GitPort {
    events: Vec<String>,
    fail_after_copy: bool,
    change_during_check: bool,
}
impl Port for GitPort {
    fn leaf(&mut self, _op: &str) -> Result<Value> {
        unreachable!()
    }
    fn command(&mut self, program: &str, args: &[String], _log: Option<&Path>) -> Result<String> {
        self.events.push(args.join(" "));
        assert_eq!(program, "git");
        let output = Command::new(program).args(args).output()?;
        if self.fail_after_copy {
            return Err("工作树创建后确认丢失".into());
        }
        if !output.status.success() {
            return Err("隔离 Git 操作失败".into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }
    fn command_at(
        &mut self,
        cwd: &Path,
        _program: &str,
        args: &[String],
        _log: Option<&Path>,
    ) -> Result<String> {
        self.events.push(args.join(" "));
        if self.change_during_check {
            fs::write(cwd.join("drift.txt"), "漂移")?;
        }
        Ok(String::new())
    }
    fn ready(&mut self, _url: &str) -> Result<bool> {
        unreachable!()
    }
    fn sleep(&mut self, _duration: std::time::Duration) -> Result<()> {
        unreachable!()
    }
    fn storage(&mut self, _table: &Value) -> Result<()> {
        unreachable!()
    }
    fn databases(&mut self, _frozen: &crate::ci::databases::snapshot::Frozen) -> Result<()> {
        unreachable!()
    }
}

#[test]
fn copies_dirty_patch_and_untracked_bytes_then_rejects_source_drift() {
    let repo = repository();
    fs::write(
        repo.path().join("Cargo.toml"),
        "[workspace]\nmembers=[]\n# modified\n",
    )
    .unwrap();
    fs::write(repo.path().join("new.txt"), "来源内容").unwrap();
    let captured = snapshot::capture(repo.path()).unwrap();
    let destination = repo.path().join(".local-tests/复制 工作树");
    let mut port = GitPort {
        events: vec![],
        fail_after_copy: false,
        change_during_check: false,
    };
    snapshot::copy(repo.path(), &destination, &captured, &mut port).unwrap();
    assert_eq!(
        fs::read(destination.join("new.txt")).unwrap(),
        fs::read(repo.path().join("new.txt")).unwrap()
    );
    assert_eq!(
        port.events.iter().filter(|v| v.contains(" apply ")).count(),
        2
    );
    fs::write(repo.path().join("new.txt"), "漂移").unwrap();
    let second = repo.path().join(".local-tests/漂移工作树");
    assert!(snapshot::copy(repo.path(), &second, &captured, &mut port).is_err());
}

#[test]
fn formal_rejection_has_no_writes_and_copy_ack_loss_keeps_preparing_evidence() {
    let backend = repository();
    let frontend = repository();
    let output = backend.path().join(".local-tests/business");
    let mut options = FixturePrepareOptions {
        output_dir: output.clone(),
        expected_sources: Some(crate::cli::FixtureExpectedSources {
            backend_sha: "a".repeat(40),
            frontend_sha: "b".repeat(40),
        }),
    };
    let mut port = GitPort {
        events: vec![],
        fail_after_copy: false,
        change_during_check: false,
    };
    assert!(prepare(&options, backend.path(), frontend.path(), &mut port).is_err());
    assert!(!output.exists());
    assert!(port.events.is_empty());
    options.expected_sources = None;
    port.fail_after_copy = true;
    assert!(prepare(&options, backend.path(), frontend.path(), &mut port).is_err());
    let receipt = documents::read(&output.join("fixture.json")).unwrap();
    assert_eq!(receipt["status"], "preparing");
    assert!(validate(&options, backend.path(), frontend.path()).is_err());
}

#[test]
fn read_only_generator_drift_never_prewarm_or_claims_ready() {
    let backend = repository();
    let frontend = repository();
    let log = backend.path().join(".local-tests/prepare.log");
    let path = backend.path().join(".local-tests/fixture.json");
    let mut receipt = json!({"status":"preparing"});
    documents::write_new(&path, &receipt).unwrap();
    let mut port = GitPort {
        events: vec![],
        fail_after_copy: false,
        change_during_check: true,
    };
    assert!(
        finish(
            backend.path(),
            frontend.path(),
            &mut port,
            &log,
            &path,
            &mut receipt
        )
        .is_err()
    );
    assert_eq!(documents::read(&path).unwrap()["status"], "preparing");
    assert!(!port.events.iter().any(|v| v == "xtask --help"));
}

#[test]
fn prewarm_uses_actual_browser_alias_only_after_read_only_source_check() {
    let backend = repository();
    let frontend = repository();
    let log = backend.path().join(".local-tests/prepare.log");
    let path = backend.path().join(".local-tests/fixture.json");
    let mut receipt = json!({"status":"preparing"});
    documents::write_new(&path, &receipt).unwrap();
    let mut port = GitPort {
        events: vec![],
        fail_after_copy: false,
        change_during_check: false,
    };
    finish(
        backend.path(),
        frontend.path(),
        &mut port,
        &log,
        &path,
        &mut receipt,
    )
    .unwrap();
    assert!(port.events[0].ends_with("resource --package order-business"));
    assert_eq!(port.events[1], "xtask --help");
    assert_eq!(documents::read(&path).unwrap()["status"], "ready");
}

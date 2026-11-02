use super::*;

#[test]
fn mysql_copy_preserves_raw_bytes_and_never_overwrites_foreign_credentials() {
    let root = crate::workspace::root_dir();
    let directory = local_directory();
    let source = directory.path().join("source.cnf");
    let target = directory.path().join("target.cnf");
    let bytes = b"[client]\r\npassword=private-sentinel\r\n";
    fs::write(&source, bytes).unwrap();
    files::copy_mysql(&root, &source, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), bytes);
    files::copy_mysql(&root, &source, &target).unwrap();
    fs::write(&target, b"foreign").unwrap();
    assert!(files::copy_mysql(&root, &source, &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"foreign");
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[test]
fn private_publication_rejects_overwrite_without_changing_existing_file() {
    let directory = local_directory();
    let path = directory.path().join("administrator.toml");
    files::write_private(&path, b"private-current-password").unwrap();
    assert!(files::write_private(&path, b"replacement").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"private-current-password");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn secrets_remain_bound_to_original_bytes_and_failed_sets_are_rejected() {
    let directory = local_directory();
    let secrets = directory
        .path()
        .join(".local-tests/reference-fixture/secrets");
    fs::create_dir_all(&secrets).unwrap();
    for name in [
        "mysql-client.cnf",
        "rustfs-access-key.txt",
        "rustfs-secret-key.txt",
        "redis-password.txt",
        "reset-admin-password.txt",
        "jwt-secret.txt",
        "metrics-token.txt",
    ] {
        fs::write(secrets.join(name), b"Aa1!private-secret\n").unwrap();
    }
    let selected = files::Secrets::open(directory.path(), directory.path(), None).unwrap();
    assert_eq!(
        selected.values().unwrap()["admin_password"],
        "Aa1!private-secret"
    );
    fs::write(secrets.join("jwt-secret.txt"), b"changed").unwrap();
    assert!(selected.check().is_err());
    fs::write(secrets.join("failed.json"), b"{}").unwrap();
    assert!(files::Secrets::open(directory.path(), directory.path(), None).is_err());
}

fn local_directory() -> tempfile::TempDir {
    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

#[test]
fn relative_output_resolves_under_root_and_occupied_directory_is_not_marked_failed() {
    let root = crate::workspace::root_dir();
    let local = local_directory();
    let target = local.path().join("environment");
    let relative = target.strip_prefix(&root).unwrap();
    assert_eq!(output_path(&root, relative).unwrap(), target);
    // 模拟输出守卫通过后、独占创建前另一调用占用目录。
    fs::create_dir(&target).unwrap();
    fs::write(target.join("marker"), b"foreign unchanged").unwrap();
    assert!(create_output(&target).is_err());
    assert_eq!(
        fs::read(target.join("marker")).unwrap(),
        b"foreign unchanged"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}

#[test]
fn owned_failure_keeps_partial_files_and_redacts_untrusted_error_chain() {
    let local = local_directory();
    let output = local.path().join("environment");
    create_output(&output).unwrap();
    files::private_permissions(&output, true).unwrap();
    files::write_private(&output.join("administrator.toml"), b"private-sentinel").unwrap();
    let error = finish(&output, Err("untrusted-password-sentinel".into())).unwrap_err();
    assert!(!error.to_string().contains("untrusted-password-sentinel"));
    let trusted = super::super::protocol::GuardError("生成秘密集收据无效");
    assert_eq!(safe_reason(&trusted), "生成秘密集收据无效");
    let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "private-io-sentinel");
    let reason = safe_reason(&io);
    assert!(reason.contains("PermissionDenied"));
    assert!(!reason.contains("private-io-sentinel"));
    assert!(error.to_string().contains("失败标记已同步"));
    assert_eq!(
        fs::read(output.join("administrator.toml")).unwrap(),
        b"private-sentinel"
    );
    assert_eq!(
        document::read(&output.join("failed.json")).unwrap(),
        json!({"status":"failed","services_started":false,"remote_writes":0})
    );
    assert!(!output.join("bootstrap.json").exists());
}

// 该 Runner 只生成测试产物字节及 Cargo 事件；不调用 build 或启动任何产物。
struct ReceiptRunner;
impl maintenance::Runner for ReceiptRunner {
    fn version(&self, root: &Path, program: &str, args: &[&str]) -> Result<String> {
        assert!(matches!(
            (program, args),
            ("cargo", ["-V"]) | ("rustc", ["-Vv"])
        ));
        maintenance::Runner::version(&maintenance::DefaultRunner, root, program, args)
    }
    fn build(
        &self,
        root: &Path,
        args: &[String],
        stdout: &Path,
        stderr: &Path,
        _: &str,
    ) -> Result<()> {
        let option = |flag| {
            args.iter()
                .position(|arg| arg == flag)
                .map(|i| &args[i + 1])
                .unwrap()
        };
        let name = option("--bin");
        let target = Path::new(option("--target-dir"));
        assert!(target.starts_with(root.join(".local-tests")));
        let executable = target
            .join("debug")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::create_dir_all(executable.parent().unwrap())?;
        fs::write(&executable, format!("non-executable fixture {name}"))?;
        fs::write(
            stdout,
            format!(
                "{}\n",
                json!({"reason":"compiler-artifact",
            "target":{"name":name,"kind":["bin"]},"manifest_path":root.join("crates/ryframe/Cargo.toml"),
            "executable":executable})
            ),
        )?;
        fs::write(stderr, b"synthetic build event, no compiler invoked\n")?;
        Ok(())
    }
}

struct PrepareFixture {
    source: crate::recovery::fixture_control::source_pair::execution::tests::Fixture,
    inputs: crate::cli::FixtureEnvironmentInputs,
    execution: PathBuf,
    explicit: PathBuf,
}
impl PrepareFixture {
    fn new() -> Self {
        let fixture =
            crate::recovery::fixture_control::source_pair::execution::tests::Fixture::new();
        fixture.prepare_environment_configuration();
        let secrets = prepare_default_secrets(&fixture.backend);
        let explicit = secrets.parent().unwrap().join("secrets-explicit");
        super::super::secrets::run(
            &crate::workspace::root_dir(),
            &FixtureEnvironmentCommand::RotateSecrets {
                fixture: fixture.receipt.clone(),
                output: explicit.clone(),
            },
        )
        .unwrap();
        let (root, initial) = super::super::protocol::tests::published_plan(&fixture);
        let execution = PathBuf::from(initial["execution_backend"].as_str().unwrap());
        let local = execution.join(".local-tests/reference-fixture");
        fs::create_dir_all(&local).unwrap();
        fs::write(
            execution.join(".local-tests/tools.toml"),
            "[generator]\ntarget_dir = '.local-tests/owned-fake-target'\n",
        )
        .unwrap();
        let build = local.join("maintenance");
        maintenance::build_with(&execution, &build, &ReceiptRunner).unwrap();
        let review_path = root.join("review.json");
        let mut review = document::read(&review_path).unwrap();
        for side in ["seed", "base", "candidate"] {
            for db in review["scopes"][side]["databases"].as_array_mut().unwrap() {
                db["connection_file"] = json!(local.join(format!("mysql-{side}.cnf")));
            }
        }
        fs::write(&review_path, serde_json::to_vec(&review).unwrap()).unwrap();
        Self {
            inputs: crate::cli::FixtureEnvironmentInputs {
                review: review_path,
                execution_source: fixture.output.join("execution-source.json"),
                maintenance_build: build.join("build.json"),
                side: crate::cli::FixtureSide::Seed,
            },
            source: fixture,
            execution,
            explicit,
        }
    }
    fn command(&self, name: &str) -> FixtureEnvironmentCommand {
        FixtureEnvironmentCommand::Prepare {
            inputs: self.inputs.clone(),
            output: self
                .execution
                .join(".local-tests/reference-fixture")
                .join(name),
            secrets_dir: Some(self.explicit.clone()),
        }
    }
}

#[test]
fn prepare_executes_full_publication_with_formal_explicit_secrets_and_private_bindings() {
    let fixture = PrepareFixture::new();
    let wrong = fixture
        .source
        .backend
        .join(".local-tests/coordinator-only-environment");
    let mut rejected = fixture.command("unused-layout");
    if let FixtureEnvironmentCommand::Prepare { output, .. } = &mut rejected {
        *output = wrong.clone();
    }
    let parent = fixture.source.backend.join(".local-tests");
    let mut before = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    before.sort();
    assert!(run(&fixture.source.backend, &rejected).is_err());
    assert!(!wrong.exists());
    let mut after = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    after.sort();
    assert_eq!(before, after);
    let review = document::read(&fixture.inputs.review).unwrap();
    for side in [crate::cli::FixtureSide::Seed, crate::cli::FixtureSide::Base] {
        let mut command = fixture.command(&format!("environment-{}", side.as_str()));
        let FixtureEnvironmentCommand::Prepare { inputs, output, .. } = &mut command else {
            unreachable!()
        };
        inputs.side = side;
        let name = inputs.side.as_str();
        let output = output.clone();
        let mut rendered = false;
        let result = run_with(&fixture.source.backend, &command, |root| {
            assert!(root.join("config/app.test.toml").is_file());
            assert!(root.join("administrator.toml").is_file());
            assert!(!root.join("bootstrap.json").exists());
            rendered = true;
            Ok(())
        })
        .unwrap();
        assert!(rendered);
        let settings = assert_prepared_environment(&fixture, &output, &result);
        let selected = &review["scopes"][name];
        assert_eq!(settings["scope_id"].as_str(), selected["scope_id"].as_str());
        assert_eq!(
            settings["jobs"]["scheduler_enabled"].as_bool(),
            Some(name == "seed")
        );
        let databases = selected["databases"].as_array().unwrap();
        let control = databases
            .iter()
            .find(|db| db["key"] == "shared-control")
            .unwrap();
        assert_eq!(
            settings["database"]["primary"]["database"].as_str(),
            control["database"].as_str()
        );
        let targets = settings["tenant_data"]["targets"].as_array().unwrap();
        assert_eq!(targets.len(), 3);
        for db in databases.iter().filter(|db| db["key"] != "shared-control") {
            let target = targets
                .iter()
                .find(|target| target["key"].as_str() == db["key"].as_str())
                .unwrap();
            assert_eq!(target["database"].as_str(), db["database"].as_str());
            assert_eq!(target["mode"].as_str(), db["mode"].as_str());
        }
        let bootstrap = fs::read(output.join("bootstrap.json")).unwrap();
        assert!(run(&fixture.source.backend, &command).is_err());
        assert_eq!(fs::read(output.join("bootstrap.json")).unwrap(), bootstrap);
        assert!(!output.join("failed.json").exists());
    }
}

fn assert_prepared_environment(
    fixture: &PrepareFixture,
    output: &Path,
    result: &Value,
) -> toml::Table {
    assert_eq!(
        *result,
        json!({"status":"prepared","services_started":false,"remote_writes":0})
    );
    let bootstrap = document::read(&output.join("bootstrap.json")).unwrap();
    let environment = document::read(&output.join("environment.json")).unwrap();
    assert_eq!(environment["environment"]["APP_ENV"], "test");
    assert_eq!(bootstrap["config_dir"], json!(output.join("config")));
    assert_eq!(bootstrap["execution_backend"], json!(fixture.execution));
    assert_eq!(
        bootstrap["configuration_sha256"],
        render::digest(&fixture.execution, "test", &output.join("config")).unwrap()
    );
    let settings: toml::Table = fs::read_to_string(output.join("config/app.test.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let account: toml::Table = fs::read_to_string(output.join("administrator.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let initial = settings["bootstrap"]["admin_password"].as_str().unwrap();
    let password = account["administrator"]["password"].as_str().unwrap();
    assert_ne!(initial, password);
    assert_eq!(
        initial,
        fs::read_to_string(fixture.explicit.join("reset-admin-password.txt"))
            .unwrap()
            .trim()
    );
    let manifest = document::read(&fixture.explicit.join("secret-set.json")).unwrap();
    assert_eq!(
        bootstrap["secret_files"]["reset-admin-password.txt"],
        manifest["files"]["reset-admin-password.txt"]
    );
    assert_eq!(
        bootstrap["administrator_credentials"],
        files::binding(&output.join("administrator.toml")).unwrap()
    );
    for secret in [initial, password, "db-private-sentinel"] {
        assert!(!bootstrap.to_string().contains(secret));
        assert!(!result.to_string().contains(secret));
        assert!(!environment.to_string().contains(secret));
    }
    assert_eq!(
        fs::read(output.join("config/app.toml")).unwrap(),
        fs::read(fixture.execution.join("config/app.toml")).unwrap()
    );
    for name in [
        "rustfs-access-key.txt",
        "rustfs-secret-key.txt",
        "redis-password.txt",
        "jwt-secret.txt",
        "metrics-token.txt",
    ] {
        let secret = fs::read_to_string(fixture.explicit.join(name)).unwrap();
        assert!(!bootstrap.to_string().contains(secret.trim()));
        assert!(!result.to_string().contains(secret.trim()));
        assert!(!environment.to_string().contains(secret.trim()));
    }
    assert!(!output.join("failed.json").exists());
    settings
}

#[test]
fn prepare_post_render_review_and_publication_raw_drift_leave_failed_not_bootstrap() {
    for publication in [false, true] {
        let fixture = PrepareFixture::new();
        let command = fixture.command("environment-drift");
        let FixtureEnvironmentCommand::Prepare { output, .. } = &command else {
            unreachable!()
        };
        let path = if publication {
            &fixture.inputs.execution_source
        } else {
            &fixture.inputs.review
        };
        let mut rendered = false;
        let error = run_with(&fixture.source.backend, &command, |root| {
            assert!(root.join("config/app.test.toml").is_file());
            assert!(root.join("administrator.toml").is_file());
            assert!(!root.join("bootstrap.json").exists());
            rendered = true;
            // 保持 JSON 语义，仅改变当前输入原字节，必须由正式末次绑定核验拒绝。
            let mut bytes = fs::read(path)?;
            bytes.push(b'\n');
            fs::write(path, bytes)?;
            Ok(())
        })
        .unwrap_err();
        assert!(rendered);
        assert!(output.join("failed.json").is_file());
        assert!(!output.join("bootstrap.json").exists());
        assert!(!output.join("environment.json").exists());
        assert!(output.join("config/app.test.toml").is_file());
        assert!(output.join("administrator.toml").is_file());
        assert!(!error.to_string().contains("db-private-sentinel"));
    }
}

fn prepare_default_secrets(backend: &Path) -> PathBuf {
    let secrets = backend.join(".local-tests/reference-fixture/secrets");
    fs::create_dir_all(&secrets).unwrap();
    for name in [
        "rustfs-access-key.txt",
        "rustfs-secret-key.txt",
        "redis-password.txt",
        "jwt-secret.txt",
        "metrics-token.txt",
    ] {
        fs::write(
            secrets.join(name),
            format!("Aa1!private-{name}-{}\n", "x".repeat(40)),
        )
        .unwrap();
    }
    fs::write(
        secrets.join("reset-admin-password.txt"),
        b"Initial!Only123\n",
    )
    .unwrap();
    fs::write(secrets.join("mysql-client.cnf"),
        b"[client]\nhost=127.0.0.1\nport=3306\nuser=root\npassword=db-private-sentinel\nssl-mode=DISABLED\n").unwrap();
    secrets
}

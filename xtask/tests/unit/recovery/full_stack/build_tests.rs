use super::*;
use crate::recovery::fixture_control::{file_binding, portable_path};
use crate::recovery::full_stack::{Port, context::Context};
use std::{process::Command, time::Duration};

pub(in crate::recovery) struct Fixture {
    _directory: tempfile::TempDir,
    pub(in crate::recovery) context: Context,
}
impl Fixture {
    pub(in crate::recovery) fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let parent = portable_path(&fs::canonicalize(directory.path()).unwrap());
        let root = parent.join("backend");
        fs::create_dir_all(root.join("crates/ryframe")).unwrap();
        fs::write(root.join("crates/ryframe/Cargo.toml"), "fixture").unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        fs::create_dir(root.join("tools")).unwrap();
        for side in [&root, &parent.join("frontend")] {
            fs::create_dir_all(side).unwrap();
            fs::write(side.join("README.md"), "fixture\n").unwrap();
            git(side, &["init", "--quiet"]);
            git(side, &["add", "."]);
            git(
                side,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    "fixture",
                ],
            );
        }
        let output = parent.join("runtime");
        fs::create_dir(&output).unwrap();
        Self {
            _directory: directory,
            context: Context {
                root,
                output,
                control: json!({}),
                run: "".into(),
                attempt: "".into(),
            },
        }
    }

    fn pair(&self) -> Value {
        let sources = json!({"backend":provenance::snapshot(&self.context.root).unwrap(),
            "frontend":provenance::snapshot(&self.context.root.parent().unwrap().join("frontend")).unwrap()});
        // CI 身份来自当前进程，测试不修改全局环境。
        let run = std::env::var("GITHUB_RUN_ID")
            .unwrap_or("12".into())
            .parse::<u64>()
            .unwrap();
        let attempt = std::env::var("GITHUB_RUN_ATTEMPT")
            .unwrap_or("3".into())
            .parse::<u64>()
            .unwrap();
        json!({"format_version":1,"backend_sha":sources["backend"]["head"],
            "frontend_sha":sources["frontend"]["head"],"run_id":run,"attempt":attempt,"sources":sources})
    }

    fn write_pair(&self, value: &Value) {
        documents::write_replace(&self.context.output.join("source-pair.json"), value).unwrap();
    }

    pub(in crate::recovery) fn business(&self) -> Value {
        let pair = self.pair();
        self.write_pair(&pair);
        let model = self
            .context
            .root
            .join("tools/python/fixtures/order-business/src/resources/mod.rs");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(&model, "#[derive(ResourceModel)]\n").unwrap();
        let frontend = self.context.root.parent().unwrap().join("frontend");
        let fixture = json!({"format_version":1,"fixture":"business","status":"ready",
            "fixture_sha256":file_binding(&model,"模型").unwrap()["sha256"],
            "sources":pair["sources"],"paths":{"backend":self.context.root,"frontend":frontend},
            "generated":{"backend":provenance::snapshot(&self.context.root).unwrap(),
                "frontend":provenance::snapshot(&frontend).unwrap()}});
        documents::write_new(
            &self.context.root.parent().unwrap().join("fixture.json"),
            &fixture,
        )
        .unwrap();
        fixture
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Cargo<'a> {
    context: &'a Context,
    calls: Vec<Vec<String>>,
    failure: Option<usize>,
    drift: bool,
}
impl Port for Cargo<'_> {
    fn leaf(&mut self, _: &str) -> Result<Value> {
        panic!("构建不调用 Python 底层")
    }
    fn command(&mut self, program: &str, args: &[String], _: Option<&Path>) -> Result<String> {
        assert_eq!(program, "cargo");
        self.calls.push(args.to_vec());
        if self.failure == Some(self.calls.len()) {
            return Err("指定构建失败".into());
        }
        let name = &args[args.iter().position(|s| s == "--bin").unwrap() + 1];
        let executable = self.context.output.join(format!("{name}.exe"));
        fs::write(&executable, name)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o750))?;
        }
        if self.drift {
            fs::write(self.context.root.join("README.md"), "changed")?;
        }
        Ok(format!(
            "{}\n{}\n",
            json!({"reason":"compiler-message"}),
            json!({"reason":"compiler-artifact","manifest_path":self.context.root.join("crates/ryframe/Cargo.toml"),
                "target":{"name":name,"kind":["bin"]},"executable":executable})
        ))
    }
    fn ready(&mut self, _: &str) -> Result<bool> {
        panic!("构建不访问网络")
    }
    fn sleep(&mut self, _: Duration) -> Result<()> {
        panic!("构建不重试")
    }
    fn storage(&mut self, _: &Value) -> Result<()> {
        panic!("构建不访问对象存储")
    }
    fn databases(&mut self, _: &crate::ci::databases::snapshot::Frozen) -> Result<()> {
        panic!("构建不访问数据库")
    }
}
fn cargo(context: &Context) -> Cargo<'_> {
    Cargo {
        context,
        calls: vec![],
        failure: None,
        drift: false,
    }
}

#[test]
fn all_four_roles_freeze_cargo_bytes_before_cache_reuse_and_publish_exact_bindings() {
    let fixture = Fixture::new();
    let context = &fixture.context;
    let mut port = cargo(context);
    let binaries = run(&context.root, &context.output, &mut |program, args| {
        port.command(program, args, None)
    })
    .unwrap();
    let evidence = documents::read(&context.output.join("build-evidence.json")).unwrap();
    assert_eq!(port.calls.len(), 4);
    assert_eq!(
        documents::read(&context.output.join("binaries.json")).unwrap(),
        binaries
    );
    assert!(evidence["source"].is_null());
    for role in [
        "ryframe",
        "ryframe-reset",
        "ryframe-migrate",
        "ryframe-worker",
    ] {
        let frozen = Path::new(binaries[role].as_str().unwrap());
        assert_eq!(
            frozen.parent(),
            Some(context.output.join("artifacts").as_path())
        );
        assert_eq!(fs::read(frozen).unwrap(), role.as_bytes());
        let cargo = context.output.join(format!("{role}.exe"));
        assert_ne!(frozen, cargo);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(frozen).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
        fs::write(cargo, "next workspace build").unwrap();
        assert_eq!(
            evidence["artifacts"][role],
            file_binding(Path::new(binaries[role].as_str().unwrap()), "产物").unwrap()
        );
    }
    assert_eq!(
        provenance::build(&context.root, &context.output, &binaries, &Value::Null).unwrap(),
        evidence
    );
    assert!(
        run(&context.root, &context.output, &mut |program, args| port
            .command(program, args, None))
        .is_err()
    );
    assert_eq!(port.calls.len(), 4);
}

#[test]
fn failed_build_or_changed_source_never_publishes_partial_success_or_replays() {
    for drift in [false, true] {
        let fixture = Fixture::new();
        fixture.write_pair(&fixture.pair());
        let mut port = cargo(&fixture.context);
        port.drift = drift;
        port.failure = (!drift).then_some(2);
        assert!(
            run(
                &fixture.context.root,
                &fixture.context.output,
                &mut |program, args| port.command(program, args, None)
            )
            .is_err()
        );
        assert_eq!(port.calls.len(), if drift { 4 } else { 2 });
        for name in ["binaries.json", "build-evidence.json"] {
            assert!(!fixture.context.output.join(name).exists());
        }
        assert!(
            fixture
                .context
                .output
                .join("artifacts/ryframe-reset.exe")
                .is_file()
        );
        let before = port.calls.len();
        assert!(
            run(
                &fixture.context.root,
                &fixture.context.output,
                &mut |program, args| port.command(program, args, None)
            )
            .is_err()
        );
        assert_eq!(port.calls.len(), before);
    }
}

#[test]
fn cargo_events_reject_missing_duplicate_other_workspace_and_wrong_kind() {
    let fixture = Fixture::new();
    let root = &fixture.context.root;
    let executable = fixture.context.output.join("product.exe");
    fs::write(&executable, "product").unwrap();
    let valid = json!({"reason":"compiler-artifact","target":{"name":"ryframe","kind":["bin"]},
        "manifest_path":root.join("crates/ryframe/Cargo.toml"),"executable":executable});
    assert_eq!(
        artifact(root, "ryframe", &valid.to_string()).unwrap(),
        executable
    );
    let mut wrong_kind = valid.clone();
    wrong_kind["target"]["kind"] = json!(["lib"]);
    let mut other = valid.clone();
    other["manifest_path"] = json!(root.join("README.md"));
    for output in [
        "{}".into(),
        "invalid".into(),
        format!("{valid}\n{valid}"),
        wrong_kind.to_string(),
        other.to_string(),
    ] {
        assert!(artifact(root, "ryframe", &output).is_err());
    }
}

#[test]
fn core_source_binds_clean_pair_and_rejects_changed_snapshots_and_duplicate_fields() {
    let fixture = Fixture::new();
    let context = &fixture.context;
    let pair = fixture.pair();
    fixture.write_pair(&pair);
    let source = provenance::current(&context.root, &context.output).unwrap();
    assert_eq!(source["fixture"], "core");
    assert_eq!(
        source["source_pair"]["receipt"],
        file_binding(&context.output.join("source-pair.json"), "来源").unwrap()
    );
    for (field, value) in [
        ("attempt", json!(true)),
        ("backend_sha", json!("a".repeat(40))),
        ("format_version", json!(true)),
    ] {
        let mut invalid = pair.clone();
        invalid[field] = value;
        fixture.write_pair(&invalid);
        assert!(provenance::current(&context.root, &context.output).is_err());
    }
    fs::write(
        context.output.join("source-pair.json"),
        "{\"format_version\":1,\"format_version\":1}",
    )
    .unwrap();
    assert!(provenance::current(&context.root, &context.output).is_err());
    fixture.write_pair(&pair);
    fs::write(context.root.join("README.md"), "changed").unwrap();
    assert!(provenance::current(&context.root, &context.output).is_err());
}

#[test]
fn business_source_binds_original_generated_model_and_local_pair_without_ci_claim() {
    let fixture = Fixture::new();
    let value = fixture.business();
    let context = &fixture.context;
    let source = provenance::current(&context.root, &context.output).unwrap();
    assert_eq!(source["fixture"], "business");
    assert_eq!(source["generated"], value["generated"]);
    let path = context.root.parent().unwrap().join("fixture.json");
    let pair = json!({"format_version":1,"kind":"reference-fixture-source-pair",
        "fixture_receipt":file_binding(&path,"业务来源").unwrap(),"sources":value["sources"]});
    fixture.write_pair(&pair);
    let source = provenance::current(&context.root, &context.output).unwrap();
    assert_eq!(
        source["source_pair"]["kind"],
        "reference-fixture-source-pair"
    );
    assert!(source["source_pair"].get("run_id").is_none());
    fs::write(
        context
            .root
            .join("tools/python/fixtures/order-business/src/resources/mod.rs"),
        "changed",
    )
    .unwrap();
    assert!(provenance::current(&context.root, &context.output).is_err());
}

#[test]
fn business_source_rejects_unready_wrong_frontend_and_rebound_fixture() {
    for field in ["status", "paths", "generated", "sources"] {
        let fixture = Fixture::new();
        let mut value = fixture.business();
        match field {
            "status" => value["status"] = json!("preparing"),
            "paths" => value["paths"]["frontend"] = json!(fixture.context.root),
            "generated" => value["generated"]["frontend"]["head"] = json!("a".repeat(40)),
            "sources" => value["sources"]["frontend"]["patch_sha256"] = json!("f".repeat(64)),
            _ => unreachable!(),
        }
        documents::write_replace(
            &fixture.context.root.parent().unwrap().join("fixture.json"),
            &value,
        )
        .unwrap();
        assert!(provenance::current(&fixture.context.root, &fixture.context.output).is_err());
    }
}

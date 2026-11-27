use super::*;
use crate::{cli::RuntimeSources, recovery::frontend_build};
use std::{collections::BTreeMap, process::Command};

const INVENTORY_TOOL: &str = "scripts/build-source-inventory.ts";
const DOMAINS_TOOL: &str = "scripts/build-source-domains.ts";
const LEGACY_INVENTORY_TOOL: &str = "scripts/build-source-inventory.mjs";
const JSON_TOOL: &str = "scripts/resource-render/json.ts";
const LEGACY_JSON_TOOL: &str = "scripts/resource-render/json.mjs";

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(root: &Path) {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
}

struct Fixture {
    _backend: tempfile::TempDir,
    _frontend: tempfile::TempDir,
    options: RuntimeBuildOptions,
}
impl Fixture {
    fn new() -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests/runtime-build-unit");
        fs::create_dir_all(&parent).unwrap();
        let backend = tempfile::tempdir_in(parent).unwrap();
        let root = backend.path();
        for directory in ["crates/ryframe", ".local-tests", "target"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("crates/ryframe/Cargo.toml"), "fixture manifest").unwrap();
        fs::write(root.join(".gitignore"), "/target/\n/.local-tests/\n").unwrap();
        git(root, &["init", "--quiet"]);
        git(root, &["config", "core.autocrlf", "false"]);
        commit(root);
        let front = frontend_build::tests::fixture();
        for name in frontend::TOOL_FILES {
            let path = front.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "fixture tool").unwrap();
        }
        commit(front.path());
        let head = inventory::capture(root).unwrap().source.snapshot.head;
        let front_head = inventory::capture(front.path())
            .unwrap()
            .source
            .snapshot
            .head;
        let options = RuntimeBuildOptions {
            sources: RuntimeSources {
                source_backend: root.to_owned(),
                source_frontend: front.path().to_owned(),
                adapter_contract: None,
                product_backend: None,
            },
            expected_head: head,
            expected_frontend_head: front_head,
            output: root.join(".local-tests/build.json"),
        };
        Self {
            _backend: backend,
            _frontend: front,
            options,
        }
    }
    fn root(&self) -> &Path {
        &self.options.sources.source_backend
    }
    fn frontend(&self) -> &Path {
        &self.options.sources.source_frontend
    }
    fn directory(&self) -> PathBuf {
        artifacts(&self.options.output).unwrap()
    }
}

#[derive(Default)]
struct Commands {
    calls: Vec<String>,
    fail_worker: bool,
    drift_source: bool,
    drift_context: bool,
    drift_frontend: Option<PathBuf>,
}
impl Port for Commands {
    fn command(&mut self, root: &Path, program: &str, args: &[String]) -> Result<String> {
        self.calls.push(program.into());
        if program == "node" {
            assert_eq!(args[1], "external-build");
            assert_eq!(args[2], "--source-root");
            assert_eq!(&args[4..], ["--expected-head", args[5].as_str(), "--write"]);
            let front = Path::new(&args[3]);
            assert_eq!(front, root);
            let inventory = inventory::inventory_value(&inventory::capture(front)?);
            frontend_build::tests::write_receipt(front, &args[5], &inventory);
            return Ok(String::new());
        }
        assert_eq!(program, "cargo");
        let name = args[args.iter().position(|arg| arg == "--bin").unwrap() + 1].clone();
        if self.fail_worker && name == "ryframe-worker" {
            return Err("fixture cargo failure".into());
        }
        if let Some(frontend) = &self.drift_frontend {
            fs::write(
                frontend.join("dist/index.html"),
                "changed while backend builds",
            )?;
        }
        let executable = root.join("target").join(format!("{name}.exe"));
        fs::write(&executable, name.as_bytes())?;
        if self.drift_source {
            fs::write(root.join("crates/ryframe/Cargo.toml"), "changed")?;
        }
        Ok(json!({"reason":"compiler-artifact","manifest_path":root.join("crates/ryframe/Cargo.toml"),
            "target":{"kind":["bin"],"name":name},"executable":executable}).to_string())
    }
    fn context(&mut self, _: &Path) -> Result<Value> {
        let mut environment = BTreeMap::new();
        if self.drift_context && self.calls.iter().any(|command| command == "cargo") {
            environment.insert("CARGO_BUILD_JOBS".into(), "7".into());
        }
        bridge::build_context::from_inputs(
            "cargo fixture",
            "rustc fixture\nhost: fixture-target",
            &environment,
            4,
        )
    }
}

#[test]
fn build_publishes_two_owned_copies_and_same_four_outputs_then_rejects_copy_drift() {
    let fixture = Fixture::new();
    let mut port = Commands::default();
    let result = run_with(
        fixture.root(),
        fixture.frontend(),
        &fixture.options,
        &mut port,
    )
    .unwrap();
    assert_eq!(result.as_object().unwrap().len(), 4);
    assert_eq!(result["frontend_action"], "built");
    assert_eq!(port.calls, ["node", "cargo", "cargo"]);
    let receipt = document::read(&fixture.options.output).unwrap();
    for role in ["api", "worker"] {
        let copy = source::path(&receipt["artifacts"][role]["executable"]).unwrap();
        assert!(copy.starts_with(fixture.directory()));
        assert!(!copy.starts_with(fixture.root().join("target")));
    }
    fs::write(fixture.root().join("target/ryframe.exe"), "later build").unwrap();
    verify(
        fixture.root(),
        &receipt,
        &fixture.options.expected_head,
        &mut port,
    )
    .unwrap();
    let copy = source::path(&receipt["artifacts"]["api"]["executable"]).unwrap();
    fs::write(copy, "tampered").unwrap();
    assert!(
        verify(
            fixture.root(),
            &receipt,
            &fixture.options.expected_head,
            &mut port
        )
        .is_err()
    );
    assert!(fixture.options.output.exists());
}

#[test]
fn failure_and_source_or_context_drift_preserve_partial_artifacts_without_success_receipt() {
    for (fail_worker, drift_source, drift_context, drift_frontend) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        let fixture = Fixture::new();
        let mut port = Commands {
            fail_worker,
            drift_source,
            drift_context,
            drift_frontend: drift_frontend.then(|| fixture.frontend().to_owned()),
            ..Commands::default()
        };
        let error = run_with(
            fixture.root(),
            fixture.frontend(),
            &fixture.options,
            &mut port,
        )
        .unwrap_err();
        assert!(!fixture.options.output.exists());
        let partial = fs::read_dir(fixture.directory()).unwrap().next();
        if cfg!(windows) && drift_frontend {
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .and_then(|error| error.raw_os_error()),
                Some(32),
                "前端证明应在首个Cargo产物生成前拒绝写入：{error}"
            );
            assert!(partial.is_none());
        } else {
            assert!(partial.is_some(), "失败阶段应保留已生成副本：{error}");
        }
        assert!(
            !fixture
                .root()
                .join(".local-tests/build/source-commit")
                .exists()
        );
        let count = port.calls.len();
        assert!(
            run_with(
                fixture.root(),
                fixture.frontend(),
                &fixture.options,
                &mut port
            )
            .is_err()
        );
        assert_eq!(port.calls.len(), count);
    }
}

#[test]
fn existing_output_or_copy_directory_rejects_before_frontend_or_cargo_and_preserves_bytes() {
    for existing_receipt in [true, false] {
        let fixture = Fixture::new();
        if existing_receipt {
            fs::write(&fixture.options.output, b"owned evidence").unwrap();
        } else {
            fs::create_dir(fixture.directory()).unwrap();
            fs::write(fixture.directory().join("partial"), b"partial evidence").unwrap();
        }
        let mut port = Commands::default();
        assert!(
            run_with(
                fixture.root(),
                fixture.frontend(),
                &fixture.options,
                &mut port
            )
            .is_err()
        );
        assert!(port.calls.is_empty());
        let path = if existing_receipt {
            fixture.options.output.clone()
        } else {
            fixture.directory().join("partial")
        };
        assert_eq!(
            fs::read(path).unwrap(),
            if existing_receipt {
                b"owned evidence".as_slice()
            } else {
                b"partial evidence".as_slice()
            }
        );
    }
}

#[test]
fn output_must_be_ignored_inside_the_backend_before_frontend_or_cargo_runs() {
    let mut fixture = Fixture::new();
    let outside = fixture.frontend().join("outside.json");
    // 显式取消目标的忽略规则，避免本机 Git 忽略配置影响用例。
    fs::write(
        fixture.root().join(".gitignore"),
        "/target/\n!/.local-tests/\n/.local-tests/ignored.json\n!/.local-tests/unignored.json\n",
    )
    .unwrap();
    let unignored = fixture.root().join(".local-tests/unignored.json");
    for (output, reason) in [
        (outside, "路径必须位于当前后端 .local-tests"),
        (unignored, "必须位于已忽略的本地目录"),
    ] {
        fixture.options.output = output;
        let mut port = Commands::default();
        let result = run_with(
            fixture.root(),
            fixture.frontend(),
            &fixture.options,
            &mut port,
        );
        assert!(result.unwrap_err().to_string().contains(reason));
        assert!(port.calls.is_empty());
        assert!(!fixture.options.output.exists());
        assert!(!fixture.directory().exists());
    }
}

#[test]
fn frontend_reuses_exact_existing_receipt_and_rejects_legacy_layout_or_wrong_head_without_command()
{
    let fixture = Fixture::new();
    let inventory = inventory::inventory_value(&inventory::capture(fixture.frontend()).unwrap());
    frontend_build::tests::write_receipt(
        fixture.frontend(),
        &fixture.options.expected_frontend_head,
        &inventory,
    );
    let mut port = Commands::default();
    let result = frontend::run(
        fixture.frontend(),
        fixture.frontend(),
        &fixture.options.expected_frontend_head,
        &mut port,
    )
    .unwrap();
    assert_eq!(result.result["frontend_action"], "verified");
    assert!(port.calls.is_empty());
    assert!(
        frontend::run(
            fixture.frontend(),
            fixture.frontend(),
            &"0".repeat(40),
            &mut port
        )
        .is_err()
    );
    fs::remove_file(fixture.frontend().join("scripts/application-config.mjs")).unwrap();
    commit(fixture.frontend());
    let head = inventory::capture(fixture.frontend())
        .unwrap()
        .source
        .snapshot
        .head;
    assert!(frontend::run(fixture.frontend(), fixture.frontend(), &head, &mut port).is_err());
    assert!(port.calls.is_empty());
}

fn use_legacy_frontend_tools(root: &Path) {
    fs::rename(root.join(INVENTORY_TOOL), root.join(LEGACY_INVENTORY_TOOL)).unwrap();
    fs::remove_file(root.join(DOMAINS_TOOL)).unwrap();
    fs::rename(root.join(JSON_TOOL), root.join(LEGACY_JSON_TOOL)).unwrap();
}

#[test]
fn frontend_inventory_binding_uses_tool_checkout_and_preserves_existing_receipt() {
    let current = Fixture::new();
    let legacy = Fixture::new();
    use_legacy_frontend_tools(legacy.frontend());
    commit(legacy.frontend());
    for (tools, root) in [
        (current.frontend(), legacy.frontend()),
        (legacy.frontend(), current.frontend()),
    ] {
        let observed = inventory::capture(root).unwrap();
        let head = &observed.source.snapshot.head;
        frontend_build::tests::write_receipt(root, head, &inventory::inventory_value(&observed));
        let receipt = root.join("dist/.vite/restore-build.json");
        let before = fs::read(&receipt).unwrap();
        let mut port = Commands::default();
        let result = frontend::run(tools, root, head, &mut port).unwrap();
        assert_eq!(result.result["frontend_action"], "verified");
        assert!(port.calls.is_empty());
        result.verify().unwrap();
        assert_eq!(fs::read(receipt).unwrap(), before);
    }
}

#[test]
fn frontend_inventory_binding_rejects_mixed_incomplete_and_unknown_git_layouts() {
    for (absent, extra) in [
        (&[][..], Some(LEGACY_INVENTORY_TOOL)),
        (&[DOMAINS_TOOL][..], None),
        (&[INVENTORY_TOOL][..], Some(LEGACY_INVENTORY_TOOL)),
        (
            &[INVENTORY_TOOL, DOMAINS_TOOL][..],
            Some("scripts/other-inventory.ts"),
        ),
    ] {
        let fixture = Fixture::new();
        let root = fixture.frontend();
        for path in absent {
            fs::remove_file(root.join(path)).unwrap();
        }
        if let Some(path) = extra {
            fs::write(root.join(path), "fixture unregistered layout").unwrap();
        }
        commit(root);
        let observed = inventory::capture(root).unwrap();
        assert!(observed.source.snapshot.clean);
        let mut port = Commands::default();
        let error = frontend::run(root, root, &observed.source.snapshot.head, &mut port)
            .err()
            .expect("不完整或混合的工具布局必须拒绝");
        assert!(error.to_string().contains("完整且唯一的来源清单布局"));
        assert!(port.calls.is_empty());
        assert!(!root.join("dist/.vite/restore-build.json").exists());
    }
}

#[test]
fn frontend_inventory_binding_rechecks_alternate_paths_outside_the_git_inventory() {
    for (legacy, extra, diagnostic) in [
        (false, LEGACY_INVENTORY_TOOL, "未绑定的来源清单文件"),
        (true, INVENTORY_TOOL, "未绑定的来源清单文件"),
        (true, DOMAINS_TOOL, "未绑定的来源清单文件"),
        (false, LEGACY_JSON_TOOL, "未绑定的 JSON 解析文件"),
        (true, JSON_TOOL, "未绑定的 JSON 解析文件"),
    ] {
        let fixture = Fixture::new();
        let root = fixture.frontend();
        if legacy {
            use_legacy_frontend_tools(root);
        }
        let ignore = root.join(".gitignore");
        let original = fs::read_to_string(&ignore).unwrap();
        fs::write(ignore, format!("{original}\n/{extra}\n")).unwrap();
        commit(root);
        let observed = inventory::capture(root).unwrap();
        let head = &observed.source.snapshot.head;
        let mut port = Commands::default();
        let result = frontend::run(root, root, head, &mut port).unwrap();
        assert_eq!(port.calls, ["node"]);
        result.verify().unwrap();

        fs::write(root.join(extra), "ignored alternate tool").unwrap();
        assert_eq!(inventory::capture(root).unwrap(), observed);
        let error = result.verify().unwrap_err();
        assert!(error.to_string().contains(diagnostic));
        let before = fs::read(root.join("dist/.vite/restore-build.json")).unwrap();
        let error = frontend::run(root, root, head, &mut port)
            .err()
            .expect("初始绑定也必须拒绝被 Git 忽略的路径");
        assert!(error.to_string().contains(diagnostic));
        assert_eq!(port.calls, ["node"]);
        assert_eq!(
            fs::read(root.join("dist/.vite/restore-build.json")).unwrap(),
            before
        );
    }
}

#[test]
fn reader_keeps_original_target_receipts_and_rejects_unknown_fields_wrong_commands_or_roles() {
    let fixture = Fixture::new();
    let mut port = Commands::default();
    fs::create_dir(fixture.directory()).unwrap();
    let mut receipt = backend(fixture.root(), &fixture.directory(), &mut port).unwrap();
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        receipt["artifacts"][role]["executable"] =
            json!(fixture.root().join(format!("target/{name}.exe")));
    }
    let bytes = serde_json::to_vec(&receipt).unwrap();
    verify(
        fixture.root(),
        &receipt,
        &fixture.options.expected_head,
        &mut port,
    )
    .unwrap();
    assert_eq!(serde_json::to_vec(&receipt).unwrap(), bytes);
    for mutation in ["extra", "command", "role", "product", "tools", "full"] {
        let mut changed = receipt.clone();
        match mutation {
            "extra" => changed["artifacts"]["api"]["cargo_executable"] = json!("unexpected"),
            "command" => changed["artifacts"]["api"]["command"][0] = json!("other-cargo"),
            "product" => changed["sources"]["product"]["api"]["sha256"] = json!("0".repeat(64)),
            "tools" => changed["sources"]["tools"]["sha256"] = json!("0".repeat(64)),
            "full" => {
                changed["sources"]["full"]["source"]["snapshot"]["head"] =
                    json!(crate::recovery::b0::BACKEND)
            }
            _ => {
                changed["artifacts"]
                    .as_object_mut()
                    .unwrap()
                    .remove("worker");
            }
        }
        assert!(
            verify(
                fixture.root(),
                &changed,
                &fixture.options.expected_head,
                &mut port
            )
            .is_err()
        );
    }
    let before = inventory::capture(fixture.root()).unwrap();
    assert!(
        source::registered(
            fixture.root(),
            fixture.root(),
            "short",
            &Value::Null,
            &Value::Null
        )
        .is_err()
    );
    assert!(
        source::registered(
            fixture.root(),
            fixture.root(),
            &fixture.options.expected_head,
            &Value::Null,
            &json!(fixture.root())
        )
        .is_err()
    );
    assert_eq!(inventory::capture(fixture.root()).unwrap(), before);
    assert!(
        source::registered(
            fixture.root(),
            fixture.root(),
            &"0".repeat(40),
            &Value::Null,
            &Value::Null
        )
        .is_err()
    );
    fs::write(
        fixture.root().join("crates/ryframe/Cargo.toml"),
        "dirty before build",
    )
    .unwrap();
    assert!(
        source::registered(
            fixture.root(),
            fixture.root(),
            &fixture.options.expected_head,
            &Value::Null,
            &Value::Null
        )
        .is_err()
    );
}

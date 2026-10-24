use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        export::transport::{CommandOutput, CommandSpec, Transport},
        json as json_io,
        target_verify::{
            database::{self, model},
            input::TargetContext,
        },
    },
};

const SCOPE: &str = "database-native-fixture";
const KEYS: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];
const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";

struct Fixture {
    _directory: tempfile::TempDir,
    context: TargetContext,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("target-database-native-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let root = directory.path();
        let execution = root.join("execution");
        fs::create_dir(&execution).unwrap();
        for relative in [
            "sql/ryframe_config.sql",
            "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
        ] {
            let path = execution.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(backend.join(relative), path).unwrap();
        }
        let generated = execution.join("crates/ryframe-tenant-db/src/generated/catalog.rs");
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        fs::write(
            generated,
            "pub static GENERATED_TENANT_DATA_TABLES: &[TenantDataTableDescriptor] = &[];\n",
        )
        .unwrap();
        let defaults = root.join("mysql.cnf");
        fs::write(&defaults, "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n").unwrap();
        let databases = KEYS.iter().map(|key| json!({"key": key, "kind": if *key == "shared-control" { "combined" } else { "tenant" },
            "mode": if key.starts_with("dedicated-") { "dedicated" } else { "shared" }, "server_uuid": UUID,
            "database": format!("physical_{}", key.replace('-', "_")), "defaults_file": defaults, "defaults_sha256": digest::file(&defaults).unwrap().sha256})).collect::<Vec<_>>();
        let target = json!({"scope_id": SCOPE, "databases": databases, "s3": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1", "access_key": "access-fixture", "secret_key": "secret-fixture"}});
        let config = execution.join("config");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("app.toml"), config_text(&databases)).unwrap();
        fs::write(config.join("app.test.toml"), "# 明确选择测试环境\n").unwrap();
        let mut selected = target.clone();
        selected["runtime_dir"] = json!(root.join("runtime"));
        selected["api_url"] = json!("http://127.0.0.1:18210");
        for db in selected["databases"].as_array_mut().unwrap() {
            db["host"] = json!("127.0.0.1");
            db["port"] = json!(3306);
        }
        let review = json!({"scopes": {"seed": selected}});
        let review_path = root.join("review.json");
        json_io::write_new(&review_path, &review).unwrap();
        let mut review_binding = bound(&review_path);
        review_binding["canonical_sha256"] = json!(digest::plan_hash(&review).unwrap());
        let tool = root.join("tool.exe");
        fs::write(&tool, "explicit-fake-tool").unwrap();
        let maintenance = json!({"source": {"snapshot": {"head": "a".repeat(40)}, "worktree_fingerprint": format!("sha256:{}", "b".repeat(64))},
            "artifacts": {"tenant-data": {"executable": tool, "sha256": digest::file(&tool).unwrap().sha256}}});
        let build = root.join("build.json");
        json_io::write_new(&build, &maintenance).unwrap();
        let request = json!({"target": target, "side": "seed", "review": review_binding, "maintenance_build": bound(&build),
            "execution_backend": {"path": execution}, "config_dir": config,
            "tools": {"mysql": {"path": tool, "sha256": digest::file(&tool).unwrap().sha256}}});
        let environment = BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("MYSQL_PWD".into(), "must-remove".into()),
        ]);
        let observation_dir = root.join("observations");
        fs::create_dir(&observation_dir).unwrap();
        let context = TargetContext {
            backend,
            run_dir: root.join("run"),
            manifest: json!({}),
            target_dir: root.join("target"),
            initialized_path: root.join("initialized.json"),
            initialized_binding: json!({}),
            initialized: json!({}),
            request_path: root.join("request.json"),
            request,
            environment,
            observation_dir,
        };
        Self {
            _directory: directory,
            context,
        }
    }

    fn database(&self, key: &str) -> Value {
        self.context
            .databases()
            .unwrap()
            .into_iter()
            .find(|db| db["key"] == key)
            .unwrap()
    }

    fn raw(&self, key: &str) -> Value {
        let db = self.database(key);
        let combined = db["kind"] == "combined";
        let (_, expected, kept) =
            model::table_sets(&self.context.execution_backend().unwrap(), combined).unwrap();
        let table = |name: &String| {
            let rows = if name == "ryframe_resource_ownership" {
                if combined { 2 } else { 1 }
            } else if kept.contains(name) || name == "biz_tenant_target_slot" {
                1
            } else {
                0
            };
            json!({"table": name, "rows": rows, "sha256": digest::plan_hash(&json!({"table": name, "rows": rows})).unwrap()})
        };
        json!({"scope_id": SCOPE, "control_schema_fingerprint": "c".repeat(16), "tenant_schema_fingerprint": "d".repeat(64),
            "target": {"database": {"key": key, "kind": db["kind"], "database": db["database"], "server_uuid": UUID, "shared": db["mode"] == "shared", "placements": [],
                "tables": expected.difference(&kept).map(table).collect::<Vec<_>>()}, "preserved_tables": kept.iter().map(table).collect::<Vec<_>>()}})
    }
}

fn config_text(databases: &[Value]) -> String {
    let mut text = format!(
        "scope_id={}\n[database.primary]\nhost=\"127.0.0.1\"\nport=3306\ndatabase={}\nusername=\"fixture\"\npassword=\"fixture-secret\"\ntls_mode=\"disabled\"\n[object_storage]\nbackend=\"rustfs\"\nendpoint=\"http://127.0.0.1:29200\"\nregion=\"us-east-1\"\naccess_key=\"access-fixture\"\nsecret_key=\"secret-fixture\"\nuse_ssl=false\n",
        json!(SCOPE),
        databases[0]["database"]
    );
    for db in &databases[1..] {
        text.push_str(&format!("[[tenant_data.targets]]\nkey={}\nkind=\"mysql\"\nmode={}\nhost=\"127.0.0.1\"\nport=3306\ndatabase={}\nusername=\"fixture\"\npassword=\"fixture-secret\"\ntls_mode=\"disabled\"\n", db["key"], db["mode"], db["database"]));
    }
    text.push_str("\n[auth]\njwt_secret=\"jwt-fixture-private\"\n[bootstrap]\nadmin_password=\"Strong!Admin123\"\n[redis]\npassword=\"redis-fixture-private\"\n");
    text
}

fn bound(path: &Path) -> Value {
    let digest = digest::file(path).unwrap();
    json!({"path": path, "bytes": digest.bytes, "sha256": digest.sha256})
}

struct Fake<'a> {
    fixture: &'a Fixture,
    effect: &'a str,
    inventories: Cell<usize>,
    identities: Cell<usize>,
    owners: Cell<usize>,
    order: RefCell<Vec<String>>,
}

impl<'a> Fake<'a> {
    fn new(fixture: &'a Fixture, effect: &'a str) -> Self {
        Self {
            fixture,
            effect,
            inventories: Cell::new(0),
            identities: Cell::new(0),
            owners: Cell::new(0),
            order: RefCell::new(Vec::new()),
        }
    }

    fn cli(&self, spec: &CommandSpec) -> Result<Vec<u8>> {
        let count = self.inventories.get() + 1;
        self.inventories.set(count);
        let key = &spec.args[2];
        self.order.borrow_mut().push(key.clone());
        let mut raw = self.fixture.raw(key);
        if self.effect == "round" && count == 5 {
            raw["target"]["database"]["placements"] =
                json!([{"tenant_id": "17", "generation": 1, "switch_token": "new-token"}]);
        }
        if self.effect == "schema" && count == 2 {
            raw["control_schema_fingerprint"] = json!("e".repeat(16));
        }
        json_io::write_new(Path::new(&spec.args[4]), &raw)?;
        if self.effect == "extra" && count == 8 {
            fs::write(spec.work_dir.join("injected.json"), "{}")?;
        }
        if self.effect == "raw-change" && count == 8 {
            fs::write(spec.work_dir.join("before-target-shared.json"), "{}")?;
        }
        if self.effect == "binding" && count == 1 {
            let config =
                PathBuf::from(self.fixture.context.request["config_dir"].as_str().unwrap())
                    .join("app.toml");
            let mut content = fs::read_to_string(&config)?;
            content.push_str("\n# 采集期间配置变化\n");
            fs::write(config, content)?;
        }
        Ok(b"fixture-secret secret-fixture access-fixture jwt-fixture-private Strong!Admin123 redis-fixture-private".to_vec())
    }
}

impl Transport for Fake<'_> {
    fn s3(
        &self,
        _: crate::recovery::clone::native::s3::Request,
    ) -> crate::Result<serde_json::Value> {
        panic!("该测试不允许对象存储调用")
    }

    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        assert_eq!(spec.cwd, self.fixture.context.execution_backend().unwrap());
        assert!(!spec.environment.contains_key("MYSQL_PWD"));
        assert_eq!(
            spec.environment["MYSQL_TEST_LOGIN_FILE"],
            spec.work_dir.join("unused-login.cnf").display().to_string()
        );
        let mut stdout = if spec
            .args
            .first()
            .is_some_and(|arg| arg == "target-inventory")
        {
            self.cli(&spec)?
        } else {
            let name = spec
                .args
                .iter()
                .find_map(|arg| arg.strip_prefix("--database="))
                .unwrap();
            let db = self
                .fixture
                .context
                .databases()?
                .into_iter()
                .find(|db| db["database"] == name)
                .unwrap();
            if spec.args.last().unwrap() == "SELECT @@server_uuid, DATABASE();" {
                self.identities.set(self.identities.get() + 1);
                if self.effect == "both-empty"
                    || (["empty", "empty-stderr"].contains(&self.effect)
                        && self.identities.get() == 1)
                {
                    Vec::new()
                } else if self.effect == "whitespace" {
                    b"\n".to_vec()
                } else {
                    format!(
                        "{}\t{name}\n",
                        if self.effect == "identity" {
                            "wrong-uuid"
                        } else {
                            UUID
                        }
                    )
                    .into_bytes()
                }
            } else {
                self.owners.set(self.owners.get() + 1);
                let scope = if self.effect == "owner" {
                    "wrong-scope"
                } else {
                    SCOPE
                };
                model::expected_owners(scope, db["kind"] == "combined")
                    .iter()
                    .map(|owner| {
                        format!(
                            "{}\t{}\t{}\n",
                            owner["resource_kind"].as_str().unwrap(),
                            scope,
                            owner["marker"].as_str().unwrap()
                        )
                    })
                    .collect::<String>()
                    .into_bytes()
            }
        };
        use crate::recovery::clone::native::export::transport::StdoutTarget;
        if let StdoutTarget::HeldFile { file, .. } = &spec.stdout {
            std::io::Write::write_all(&mut file.try_clone()?, &stdout)?;
            stdout.clear();
        }
        Ok(CommandOutput {
            status: success(),
            stdout,
            stderr: if self.effect == "empty-stderr" {
                b"fixture-secret".to_vec()
            } else {
                Vec::new()
            },
        })
    }
}

#[cfg(windows)]
fn success() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}
#[cfg(unix)]
fn success() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[test]
fn two_rounds_eight_raw_outputs_and_physical_images() {
    let fixture = Fixture::new();
    let transport = Fake::new(&fixture, "none");
    let checkpoints = Cell::new(0);
    let captured = database::capture_with(&fixture.context, &transport, &|| {
        checkpoints.set(checkpoints.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.owners.get(), 24);
    assert_eq!(*transport.order.borrow(), KEYS.repeat(2));
    assert!(checkpoints.get() > 96);
    let receipt = json_io::read(Path::new(captured.receipt["path"].as_str().unwrap())).unwrap();
    assert_eq!(receipt["inventories"].as_object().unwrap().len(), 8);
    assert_eq!(
        receipt["observations"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["dedicated-a", "dedicated-b", "shared", "shared-control"]
    );
    assert_eq!(
        captured.observations.keys().cloned().collect::<Vec<_>>(),
        [
            "physical_dedicated_a",
            "physical_dedicated_b",
            "physical_shared",
            "physical_shared_control"
        ]
    );
    assert_eq!(captured.evidence_files.len(), 139);
    assert_eq!(receipt.as_object().unwrap().len(), 14);
    for flag in [
        "producer_stopped_proven",
        "fresh_target_proven",
        "target_ready",
        "clone_verified",
        "restore_qualified",
    ] {
        assert_eq!(receipt[flag], false);
    }
    for evidence in captured.evidence_files {
        let path = fixture
            .context
            .observation_dir
            .join("inventory")
            .join(evidence["path"].as_str().unwrap());
        let digest = digest::file(&path).unwrap();
        assert_eq!(evidence["bytes"], digest.bytes);
        assert_eq!(evidence["sha256"], digest.sha256);
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("command-")
        {
            let value = fs::read_to_string(path).unwrap();
            for secret in [
                "fixture-secret",
                "secret-fixture",
                "access-fixture",
                "jwt-fixture-private",
                "Strong!Admin123",
                "redis-fixture-private",
            ] {
                assert!(!value.contains(secret), "命令收据必须脱敏所有文件凭据");
            }
        }
    }
}

#[test]
fn invalid_fresh_config_paths_and_toml_fail_before_any_resource_command() {
    for case in [
        "missing",
        "retired",
        "relative",
        "coordinator",
        "malformed",
        "selected-missing",
    ] {
        let mut fixture = Fixture::new();
        let request = &mut fixture.context.request;
        let directory = PathBuf::from(request["config_dir"].as_str().unwrap());
        match case {
            "missing" => {
                request.as_object_mut().unwrap().remove("config_dir");
            }
            "retired" => {
                let value = request
                    .as_object_mut()
                    .unwrap()
                    .remove("config_dir")
                    .unwrap();
                request["configuration_directory"] = value;
            }
            "relative" => request["config_dir"] = json!("config"),
            "coordinator" => {
                let config = fixture._directory.path().join("config");
                fs::create_dir(&config).unwrap();
                for name in ["app.toml", "app.test.toml"] {
                    fs::copy(directory.join(name), config.join(name)).unwrap();
                }
                request["config_dir"] = json!(config);
            }
            "malformed" => {
                fs::write(directory.join("app.test.toml"), "[invalid\n").unwrap();
            }
            "selected-missing" => {
                fs::remove_file(directory.join("app.test.toml")).unwrap();
            }
            _ => unreachable!(),
        }
        let fake = Fake::new(&fixture, "none");
        assert!(
            database::capture_with(&fixture.context, &fake, &|| Ok(())).is_err(),
            "{case}"
        );
        assert_eq!(fake.inventories.get(), 0, "{case}");
        assert_eq!(fake.identities.get(), 0, "{case}");
        assert_eq!(fake.owners.get(), 0, "{case}");
        assert!(fake.order.borrow().is_empty(), "{case}");
    }
}

#[test]
fn drift_unknown_evidence_and_actual_database_mismatch_fail_closed() {
    for effect in [
        "round",
        "schema",
        "extra",
        "raw-change",
        "binding",
        "identity",
        "owner",
        "whitespace",
        "empty-stderr",
        "both-empty",
    ] {
        let fixture = Fixture::new();
        assert!(
            database::capture_with(&fixture.context, &Fake::new(&fixture, effect), &|| Ok(()))
                .is_err(),
            "{effect}"
        );
        let output = fixture.context.observation_dir.join("inventory");
        assert!(!output.join("inventory.json").exists(), "{effect}");
        assert!(output.join("failure.json").is_file(), "{effect}");
        if [
            "identity",
            "owner",
            "whitespace",
            "empty-stderr",
            "both-empty",
        ]
        .contains(&effect)
        {
            let failure = json_io::read(&output.join("failure.json")).unwrap();
            assert_eq!(failure["error_type"], "DatabaseVerificationError");
            assert!(
                failure["database_verification"]
                    .get("response_bytes")
                    .is_some()
            );
            assert!(!failure.to_string().contains("fixture-secret"));
        }
    }
}

#[test]
fn only_exact_empty_success_gets_one_identity_retry() {
    let fixture = Fixture::new();
    let fake = Fake::new(&fixture, "empty");
    let captured = database::capture_with(&fixture.context, &fake, &|| Ok(())).unwrap();
    assert_eq!(fake.identities.get(), 17);
    assert_eq!(fake.inventories.get(), 8);
    assert_eq!(
        captured
            .evidence_files
            .iter()
            .filter(|item| item["path"]
                .as_str()
                .unwrap()
                .starts_with("mysql-identity-retry-"))
            .count(),
        1
    );
}

#[test]
fn checkpoint_error_is_returned_and_no_inventory_is_published() {
    let fixture = Fixture::new();
    let fake = Fake::new(&fixture, "none");
    let calls = Cell::new(0);
    let result = database::capture_with(&fixture.context, &fake, &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 11 {
            Err("explicit checkpoint failure".into())
        } else {
            Ok(())
        }
    });
    let error = result.err().unwrap();
    assert!(error.to_string().contains("CheckpointError"));
    assert!(
        error
            .source()
            .unwrap()
            .to_string()
            .contains("explicit checkpoint failure")
    );
    assert!(
        !fixture
            .context
            .observation_dir
            .join("inventory/inventory.json")
            .exists()
    );
}

fn python_observation_adapter(
    fixture: &Fixture,
    raw: &Value,
    declared: &Value,
    owners: &[Value],
) -> Value {
    let input = fixture.context.observation_dir.join("oracle-input.json");
    json_io::write_replace(
        &input,
        &json!({"raw": raw, "declared": declared, "owners": owners}),
    )
    .unwrap();
    let script = r#"import json,sys,subprocess
from pathlib import Path
from dataclasses import asdict
from devex_clone_inventory import observation_from_inventory
v=json.loads(Path(sys.argv[2]).read_text(encoding='utf-8'))
try:
    result=observation_from_inventory(Path(sys.argv[1]),v['raw'],v['declared'],'database-native-fixture',tuple(v['owners']))
    print(json.dumps({'ok':True,'value':asdict(result)},ensure_ascii=False))
except ValueError:
    print(json.dumps({'ok':False}))
"#;
    let output = crate::target_configuration_fixture::python_command(script)
        .arg(fixture.context.execution_backend().unwrap())
        .arg(input)
        .env(
            "PYTHONPATH",
            crate::workspace::root_dir().join("tools/python"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn python_observation_adapter_preserves_all_four_target_results() {
    let fixture = Fixture::new();
    for key in KEYS {
        let db = fixture.database(key);
        let raw = fixture.raw(key);
        let owners = model::expected_owners(SCOPE, key == "shared-control");
        let native = model::observation(
            &fixture.context.execution_backend().unwrap(),
            &raw,
            &db,
            SCOPE,
            &owners,
        )
        .unwrap();
        // 同一原生模型的跨进程适配契约，核对 Python 返回类型与完整结果。
        let python = python_observation_adapter(&fixture, &raw, &db, &owners);
        assert_eq!(python, json!({"ok": true, "value": native}));
    }
}

fn invalid_inventories(fixture: &Fixture) -> Vec<Value> {
    let original = fixture.raw("shared");
    let mut result = Vec::new();
    for rows in [json!(-1), json!(true), json!(1.5)] {
        let mut raw = original.clone();
        raw["target"]["database"]["tables"][0]["rows"] = rows;
        result.push(raw);
    }
    for sha in ["a".repeat(63), "A".repeat(64), "z".repeat(64)] {
        let mut raw = original.clone();
        raw["target"]["database"]["tables"][0]["sha256"] = json!(sha);
        result.push(raw);
    }
    for placements in [
        json!([{"tenant_id": "1", "generation": true, "switch_token": "token"}]),
        json!([{"tenant_id": "1", "generation": 0, "switch_token": "token"}]),
        json!([{"tenant_id": "1", "generation": -1, "switch_token": "token"}]),
        json!([{"tenant_id": "1", "generation": 1, "switch_token": "token"}, {"tenant_id": "1", "generation": 2, "switch_token": "next"}]),
    ] {
        let mut raw = original.clone();
        raw["target"]["database"]["placements"] = placements;
        result.push(raw);
    }
    let mut unknown = original.clone();
    unknown["target"]["database"]["tables"][0]["table"] = json!("unreviewed_table");
    result.push(unknown);
    let mut missing = original.clone();
    missing["target"]["database"]["tables"]
        .as_array_mut()
        .unwrap()
        .pop();
    result.push(missing);
    let mut overlap = original.clone();
    let kept = overlap["target"]["preserved_tables"][0].clone();
    overlap["target"]["database"]["tables"]
        .as_array_mut()
        .unwrap()
        .push(kept);
    result.push(overlap);
    let mut missing_kept = original.clone();
    missing_kept["target"]["preserved_tables"]
        .as_array_mut()
        .unwrap()
        .pop();
    result.push(missing_kept);
    let mut duplicate = original.clone();
    let first = duplicate["target"]["database"]["tables"][0].clone();
    duplicate["target"]["database"]["tables"]
        .as_array_mut()
        .unwrap()
        .push(first);
    result.push(duplicate);
    let mut owner_rows = original;
    owner_rows["target"]["preserved_tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|table| table["table"] == "ryframe_resource_ownership")
        .unwrap()["rows"] = json!(2);
    result.push(owner_rows);
    result
}

#[test]
fn malformed_tables_and_placements_are_rejected() {
    let fixture = Fixture::new();
    let db = fixture.database("shared");
    let owners = model::expected_owners(SCOPE, false);
    let cases = invalid_inventories(&fixture);
    assert_eq!(cases.len(), 16);
    for raw in cases {
        assert!(
            model::observation(
                &fixture.context.execution_backend().unwrap(),
                &raw,
                &db,
                SCOPE,
                &owners
            )
            .is_err()
        );
    }
}

#[test]
fn fresh_only_restricts_biz_rows_and_external_fence_slot() {
    let fixture = Fixture::new();
    let db = fixture.database("shared");
    let owners = model::expected_owners(SCOPE, false);
    let image = model::observation(
        &fixture.context.execution_backend().unwrap(),
        &fixture.raw("shared"),
        &db,
        SCOPE,
        &owners,
    )
    .unwrap();
    assert!(model::fresh(&image, false).is_ok());
    for (name, rows) in [
        ("biz_tenant_fence", 1),
        ("biz_tenant_target_slot", 0),
        ("biz_data_scope_owner", 1),
    ] {
        let mut changed = image.clone();
        changed["tables"][name]["rows"] = json!(rows);
        assert!(model::fresh(&changed, false).is_err());
    }
    let mut control = image;
    control["tables"]["sys_user"] = json!({"rows": 123, "sha256": "a".repeat(64)});
    assert!(model::fresh(&control, true).is_ok());
}

#[test]
fn python_can_reverify_the_complete_native_receipt() {
    let fixture = Fixture::new();
    let captured =
        database::capture_with(&fixture.context, &Fake::new(&fixture, "none"), &|| Ok(())).unwrap();
    let arguments = fixture.context.observation_dir.join("reverify.json");
    json_io::write_new(
        &arguments,
        &json!({"receipt": captured.receipt, "target": fixture.context.target(), "request": fixture.context.request, "environment": fixture.context.environment}),
    )
    .unwrap();
    let script = r#"import json,sys,subprocess
from pathlib import Path
from devex_clone_inventory import verify_side_inventory,configuration
from native_s3_client import _invoke as native_invoke, _read_response as native_response
from restore_reference_plan import plan_hash
from restore_build import file_digest
v=json.loads(Path(sys.argv[3]).read_text(encoding='utf-8'))
result=verify_side_inventory(Path(sys.argv[1]),v['receipt'],v['target'],Path(sys.argv[2]))
assert len(result['receipt']['inventories'])==8
assert len(result['databases'])==4
request=v['request']
review=json.loads(Path(request['review']['path']).read_text(encoding='utf-8'))
selected=review['scopes'][request['side']]
side={**v['target'],**{key:selected[key] for key in ('runtime_dir','api_url')}}
maintenance=Path(request['maintenance_build']['path'])
expected={'side':'target','tools_plan_sha256':plan_hash({'target':side,'tools':request['tools']}),
          'configuration':configuration(Path(sys.argv[2]),v['environment'],side,evidence_root=Path(sys.argv[1]),config_dir=Path(result['binding']['configuration']['directory'])),
          'physical':native_response(*native_invoke('__source-evidence', {'operation':'physical-binding','write':False,'arguments':{'backend':sys.argv[2],'evidence_root':sys.argv[1],'source':side,'app_env':v['environment']['APP_ENV'],'config_dir':result['binding']['configuration']['directory']}},subprocess.run,may_write=False))[0]['value'],
          'maintenance':file_digest(maintenance),'source':json.loads(maintenance.read_text(encoding='utf-8'))['source']}
assert result['binding']==expected
print(json.dumps(result['receipt']))
"#;
    let output = crate::target_configuration_fixture::python_command(script)
        .arg(&fixture.context.backend)
        .arg(fixture.context.execution_backend().unwrap())
        .arg(arguments)
        .env(
            "PYTHONPATH",
            crate::workspace::root_dir().join("tools/python"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        python,
        json_io::read(Path::new(captured.receipt["path"].as_str().unwrap())).unwrap()
    );
}

fn add_business_catalog(fixture: &Fixture) {
    let execution = fixture.context.execution_backend().unwrap();
    fs::write(
        execution.join("Cargo.toml"),
        "[workspace]\nmembers=[\"business\"]\n",
    )
    .unwrap();
    let package = execution.join("business");
    fs::create_dir_all(package.join("src/generated")).unwrap();
    fs::create_dir(package.join(".ryframe")).unwrap();
    fs::write(
        package.join("Cargo.toml"),
        "[package.metadata.ryframe]\nkind=\"business\"\nmodule=\"fixture\"\n",
    )
    .unwrap();
    let table = json!({"table": "biz_fixture_item", "copy_order": 1, "tenant_column": "tenant_id", "primary_key_cursor_columns": ["id"],
        "checksum_columns": ["id", "tenant_id"], "column_types": ["bigint", "bigint"], "has_generated_columns": false,
        "foreign_key_dependencies": [], "foreign_keys": [], "schema_canonical": "fixture-schema"});
    let catalog = package.join("src/generated/tenant_catalog.json");
    let resources = package.join("src/generated/resources.json");
    json_io::write_new(&catalog, &json!({"format_version": 1, "tables": [table]})).unwrap();
    json_io::write_new(&resources, &json!({"format_version": 1, "module": "fixture", "resources": [{"database": "tenant", "table": "biz_fixture_item"}]})).unwrap();
    let managed = format!(
        "[[entries]]\nresource=\"__catalog__\"\npath=\"src/generated/tenant_catalog.json\"\ncontent_hash=\"{}\"\n[[entries]]\nresource=\"__catalog__\"\npath=\"src/generated/resources.json\"\ncontent_hash=\"{}\"\n",
        digest::file(&catalog).unwrap().sha256,
        digest::file(&resources).unwrap().sha256
    );
    fs::write(package.join(".ryframe/generated.toml"), managed).unwrap();
    let ledger = execution.join("crates/ryframe-sdk/src/migration/ledger.rs");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::copy(
        fixture
            .context
            .backend
            .join("crates/ryframe-sdk/src/migration/ledger.rs"),
        ledger,
    )
    .unwrap();
}

#[test]
fn explicit_business_catalog_requires_the_business_table_and_ledger() {
    let fixture = Fixture::new();
    add_business_catalog(&fixture);
    let db = fixture.database("shared");
    let raw = fixture.raw("shared");
    let owners = model::expected_owners(SCOPE, false);
    let backend = fixture.context.execution_backend().unwrap();
    let native = model::observation(&backend, &raw, &db, SCOPE, &owners).unwrap();
    assert!(native["tables"].get("biz_fixture_item").is_some());
    assert!(
        native["preserved"]
            .get("ryframe_business_migration")
            .is_some()
    );
    for name in ["biz_fixture_item", "ryframe_business_migration"] {
        let mut missing = raw.clone();
        for field in ["/target/database/tables", "/target/preserved_tables"] {
            missing
                .pointer_mut(field)
                .unwrap()
                .as_array_mut()
                .unwrap()
                .retain(|table| table["table"] != name);
        }
        assert!(model::observation(&backend, &missing, &db, SCOPE, &owners).is_err());
    }
}

#[test]
fn fixed_topology_rejects_missing_duplicate_and_wrong_mode_targets() {
    for effect in [
        "missing",
        "duplicate-key",
        "duplicate-database",
        "wrong-mode",
    ] {
        let mut fixture = Fixture::new();
        let databases = fixture.context.request["target"]["databases"]
            .as_array_mut()
            .unwrap();
        match effect {
            "missing" => {
                databases.pop();
            }
            "duplicate-key" => databases[1]["key"] = databases[0]["key"].clone(),
            "duplicate-database" => databases[1]["database"] = databases[0]["database"].clone(),
            "wrong-mode" => databases[1]["mode"] = json!("dedicated"),
            _ => unreachable!(),
        }
        assert!(model::databases(&fixture.context).is_err(), "{effect}");
    }
}

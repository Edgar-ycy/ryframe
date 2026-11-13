use super::*;
use crate::recovery::fixture_control::portable_path;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const TOKEN: &str = "synthetic-private-recovery-token";
const SCOPE: &str = "runtime-environment-case";

pub(in crate::recovery::runtime) struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    execution: PathBuf,
    registration: PathBuf,
    private: PathBuf,
    api: PathBuf,
    worker: PathBuf,
    ports: [u16; 3],
}

impl Fixture {
    fn new() -> Self {
        Self::with_ports([12341, 12342, 12343])
    }

    pub(in crate::recovery::runtime) fn with_ports(ports: [u16; 3]) -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("runtime-start-environment-")
            .tempdir_in(local)
            .unwrap();
        let root = portable_path(&temporary.path().canonicalize().unwrap());
        let execution = root.join("execution");
        let evidence = root.join(".local-tests/evidence");
        let registration = evidence.join("registration.json");
        let configuration = configuration_registration::sidecar(&registration).unwrap();
        let private = configuration.join("environment.json");
        let api = execution.join("api");
        let worker = configuration.join("worker");
        for role in [&api, &worker] {
            fs::create_dir_all(role).unwrap();
            fs::write(
                role.join("app.toml"),
                format!("scope_id = \"{SCOPE}\"\n[app]\nhost = \"127.0.0.1\"\n[jobs]\nmode = \"external\"\nhealth_host = \"127.0.0.1\"\n[monitor]\nmetrics_bearer_token = \"{TOKEN}\"\n"),
            )
            .unwrap();
        }
        let fixture = Self {
            _temporary: temporary,
            root,
            execution,
            registration,
            private,
            api,
            worker,
            ports,
        };
        fixture.api_toml(2, ports[0]);
        fixture.worker_toml(3, ports[1]);
        fixture.write_private(&fixture.api);
        fixture.write_registration();
        fixture
    }

    fn write_private(&self, api: &Path) {
        fs::write(
            &self.private,
            serde_json::to_vec(&json!({
                "metrics_bearer_token":TOKEN,"scope_id":SCOPE,"api_config_dir":api,
                "worker_config_dir":self.worker,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    pub(in crate::recovery::runtime) fn request(&self) -> Value {
        let registration = document::read(&self.registration, &self.root, "运行登记夹具").unwrap();
        let [api, worker, frontend] = self.ports;
        json!({"authority":{"scope_id":SCOPE,"api_endpoint":format!("http://127.0.0.1:{api}/readyz"),
            "worker_endpoint":format!("http://127.0.0.1:{worker}/readyz"),
            "frontend_endpoint":format!("http://127.0.0.1:{frontend}")},
            "registration":{"registration":document::binding(&registration)},
            "roots":{"backend_execution":self.execution},"tools":{"coordinator_root":self.root}})
    }

    pub(in crate::recovery::runtime) fn worker_directory(&self) -> &Path {
        &self.worker
    }

    fn write_registration(&self) {
        fs::write(
            &self.registration,
            serde_json::to_vec(&json!({"runtime_environment":self.environment_binding()})).unwrap(),
        )
        .unwrap();
    }

    fn rebind_private(&self, private: &Value) -> Value {
        fs::write(&self.private, serde_json::to_vec(private).unwrap()).unwrap();
        self.write_registration();
        self.request()
    }

    fn environment_binding(&self) -> Value {
        let document = document::read(&self.private, &self.root, "隔离测试环境").unwrap();
        document::binding(&document)
    }

    fn api_toml(&self, id: i64, port: u16) {
        fs::write(
            self.api.join("app.test.toml"),
            format!("snowflake_worker_id = {id}\n[app]\nport = {port}\n"),
        )
        .unwrap();
    }

    fn worker_toml(&self, id: i64, port: u16) {
        fs::write(
            self.worker.join("app.test.toml"),
            format!("snowflake_worker_id = {id}\n[jobs]\nhealth_port = {port}\n"),
        )
        .unwrap();
    }
}

#[test]
fn captures_both_exact_role_snapshots_and_existing_verifiers_consume_the_same_value() {
    let fixture = Fixture::new();
    fs::write(fixture.api.join("app.dev.toml"), b"unselected = true\n").unwrap();
    let mut request = fixture.request();
    let mut inputs = Inputs::new(&fixture.root);
    let snapshot = capture(&mut inputs, &request).unwrap();
    assert!(request.get("environment").is_none());
    assert_eq!(snapshot["document"], fixture.environment_binding());
    for (role, directory, names) in [
        (
            "api",
            &fixture.api,
            &["app.dev.toml", "app.test.toml", "app.toml"][..],
        ),
        (
            "worker",
            &fixture.worker,
            &["app.test.toml", "app.toml"][..],
        ),
    ] {
        let files = names
            .iter()
            .map(|name| {
                let bytes = fs::read(directory.join(name)).unwrap();
                (
                    *name,
                    json!({"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let environment_sha256 = hex::encode(Sha256::digest(br#"{"APP_ENV":"test"}"#));
        assert_eq!(
            snapshot["configurations"][role],
            json!({
                "directory":portable_path(&directory.canonicalize().unwrap()),
                "files":files,"environment_sha256":environment_sha256,
            })
        );
    }
    assert!(!snapshot.to_string().contains(TOKEN));
    request["environment"] = snapshot;
    verify(&mut inputs, &request).unwrap();

    fs::write(fixture.worker.join("extra.toml"), b"unselected = true\n").unwrap();
    assert!(verify(&mut inputs, &request).is_err());
    fs::remove_file(fixture.worker.join("extra.toml")).unwrap();
    fs::write(fixture.api.join("app.dev.toml"), b"unselected = fals\n").unwrap();
    assert!(verify(&mut inputs, &request).is_err());
    fs::remove_file(fixture.api.join("app.dev.toml")).unwrap();
    assert!(verify(&mut inputs, &request).is_err());
}

#[test]
fn rejects_wrong_private_binding_outside_configuration_duplicate_id_and_bad_toml() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let mut wrong = request.clone();
    wrong["registration"]["registration"]["sha256"] = json!("0".repeat(64));
    assert!(capture(&mut Inputs::new(&fixture.root), &wrong).is_err());

    let mut wrong_scope = request.clone();
    wrong_scope["authority"]["scope_id"] = json!("other-scope");
    assert!(capture(&mut Inputs::new(&fixture.root), &wrong_scope).is_err());

    fixture.worker_toml(2, 12342);
    let result = capture(&mut Inputs::new(&fixture.root), &request).unwrap_err();
    assert!(result.to_string().contains("实例编号必须不同"));
    fixture.worker_toml(3, 12343);
    assert!(capture(&mut Inputs::new(&fixture.root), &request).is_err());
    fixture.worker_toml(3, 12342);

    fs::write(fixture.api.join("app.test.toml"), format!("[{TOKEN}\n")).unwrap();
    let result = capture(&mut Inputs::new(&fixture.root), &request).unwrap_err();
    assert!(!result.to_string().contains(TOKEN));
    fixture.api_toml(2, 12341);

    let outside = fixture.root.join("unregistered");
    fs::create_dir(&outside).unwrap();
    fixture.write_private(&outside);
    assert!(capture(&mut Inputs::new(&fixture.root), &request).is_err());
    fixture.write_registration();
    assert!(capture(&mut Inputs::new(&fixture.root), &fixture.request()).is_err());
}

#[test]
fn rebound_private_document_rejects_retired_fields_nontext_token_and_relative_path() {
    let fixture = Fixture::new();
    let original = document::read(&fixture.private, &fixture.root, "隔离测试环境").unwrap();
    let request = fixture.request();
    let captured = capture(&mut Inputs::new(&fixture.root), &request).unwrap();
    assert_eq!(captured["document"], document::binding(&original));

    let with_field = |field: &str, value: Value| {
        let mut private = original.value.clone();
        private[field] = value;
        private
    };
    let fields_error = "恢复私有环境字段缺失或包含未知字段";
    for (case, private, expected) in [
        (
            "旧环境包装",
            json!({"environment": original.value}),
            fields_error,
        ),
        (
            "旧任务模式",
            with_field("APP_JOBS_MODE", json!("external")),
            fields_error,
        ),
        (
            "旧 scope 字段",
            with_field("app_scope_id", json!(SCOPE)),
            fields_error,
        ),
        (
            "额外字段",
            with_field("extra", json!("unexpected")),
            fields_error,
        ),
        (
            "布尔令牌",
            with_field("metrics_bearer_token", json!(false)),
            "恢复私有环境字段必须为字符串",
        ),
        (
            "相对 API 配置目录",
            with_field("api_config_dir", json!("relative-config")),
            "恢复静态权威路径必须为无父目录跳转的绝对路径",
        ),
    ] {
        let rebound = fixture.rebind_private(&private);
        assert_ne!(rebound["registration"], request["registration"], "{case}");
        let error = capture(&mut Inputs::new(&fixture.root), &rebound)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(TOKEN), "{case}");
        assert_eq!(error, expected, "{case}");
    }
}

#[test]
fn verify_rejects_missing_selected_configuration_and_token_drift_without_echoing_tokens() {
    const REPLACEMENT_TOKEN: &str = "synthetic-replacement-recovery-token";

    let fixture = Fixture::new();
    let mut previous = fixture.request();
    previous["environment"] = capture(&mut Inputs::new(&fixture.root), &previous).unwrap();
    verify(&mut Inputs::new(&fixture.root), &previous).unwrap();

    let selected = fixture.api.join("app.test.toml");
    let selected_bytes = fs::read(&selected).unwrap();
    fs::remove_file(&selected).unwrap();
    let missing = verify(&mut Inputs::new(&fixture.root), &previous)
        .unwrap_err()
        .to_string();
    fs::write(&selected, selected_bytes).unwrap();
    verify(&mut Inputs::new(&fixture.root), &previous).unwrap();

    for directory in [&fixture.api, &fixture.worker] {
        let common = directory.join("app.toml");
        let original = fs::read_to_string(&common).unwrap();
        assert!(original.contains(TOKEN));
        fs::write(&common, original.replace(TOKEN, REPLACEMENT_TOKEN)).unwrap();
    }
    let mismatch = verify(&mut Inputs::new(&fixture.root), &previous)
        .unwrap_err()
        .to_string();

    let mut private = document::read(&fixture.private, &fixture.root, "隔离测试环境")
        .unwrap()
        .value;
    private["metrics_bearer_token"] = json!(REPLACEMENT_TOKEN);
    let mut rebound = fixture.rebind_private(&private);
    let current = capture(&mut Inputs::new(&fixture.root), &rebound).unwrap();
    assert_ne!(previous["environment"]["document"], current["document"]);
    for role in ["api", "worker"] {
        assert_ne!(
            previous["environment"]["configurations"][role]["files"]["app.toml"],
            current["configurations"][role]["files"]["app.toml"],
        );
    }
    rebound["environment"] = previous["environment"].take();
    let drift = verify(&mut Inputs::new(&fixture.root), &rebound)
        .unwrap_err()
        .to_string();
    rebound["environment"] = current;
    verify(&mut Inputs::new(&fixture.root), &rebound).unwrap();

    for (error, expected) in [
        (missing, "目标配置目录缺少 app.test.toml"),
        (mismatch, "恢复角色配置与 scope、端点、令牌或实例编号不同"),
        (drift, "恢复启动角色配置快照发生漂移"),
    ] {
        assert!(!error.contains(TOKEN) && !error.contains(REPLACEMENT_TOKEN));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn frozen_b0_receipt_binds_private_role_files_and_only_backend_child_gets_legacy_variables() {
    use std::process::Command;

    let fixture = Fixture::new();
    let mut request = fixture.request();
    request["authority"]["frontend_sha"] = json!(crate::recovery::frontend_build::b0::HEAD);
    request["authority"]["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    request["authority"]["backend_product_sha"] = json!(crate::recovery::b0::BACKEND);
    request["authority"]["backend_execution_sha"] = json!(crate::recovery::b0::COMMIT);
    let mut inputs = Inputs::new(&fixture.root);
    let example_password = "synthetic-b0-db-password";
    for directory in [&fixture.api, &fixture.worker] {
        let file = directory.join("app.test.toml");
        let mut contents = fs::read_to_string(&file).unwrap();
        for (key, mode) in [
            ("shared", "shared"),
            ("dedicated-a", "dedicated"),
            ("dedicated-b", "dedicated"),
        ] {
            contents.push_str(&format!(
                "\n[[tenant_data.targets]]\nkey = '{key}'\nmode = '{mode}'\nkind = 'mysql'\npassword = '{example_password}'\n"
            ));
        }
        fs::write(file, contents).unwrap();
    }
    request["environment"] = capture(&mut inputs, &request).unwrap();
    let proof = request["environment"].to_string();
    assert!(!proof.contains(example_password) && !proof.contains(TOKEN));
    assert_eq!(
        request["environment"]["configurations"]["api"]["legacy"]["worker_id"],
        2
    );
    assert_eq!(
        request["environment"]["configurations"]["worker"]["legacy"]["worker_id"],
        3
    );
    let generation = fixture.root.join(".local-tests/generation");
    fs::create_dir(&generation).unwrap();
    materialize(&mut inputs, &request, &generation).unwrap();
    assert!(materialize(&mut inputs, &request, &generation).is_err());
    let projected = generation.join("b0-config-api/app.test.toml");
    let body = fs::read_to_string(&projected).unwrap();
    assert!(!body.contains(example_password) && !body.contains("snowflake_worker_id"));
    let mut product = Command::new("api-fixture");
    configure(&mut inputs, &request, &generation, "api", &mut product).unwrap();
    let environment = product.get_envs().collect::<BTreeMap<_, _>>();
    let value = |name: &str| environment[std::ffi::OsStr::new(name)].unwrap();
    assert_eq!(
        value("APP_CONFIG_DIR"),
        generation.join("b0-config-api").as_os_str()
    );
    assert_eq!(value("SNOWFLAKE_WORKER_ID"), "2");
    for variable in super::super::b0::PASSWORD_VARIABLES {
        assert_eq!(value(variable), example_password);
    }
    let mut other = Command::new("not-a-backend-fixture");
    configure(&mut inputs, &request, &generation, "frontend", &mut other).unwrap();
    assert_eq!(other.get_envs().count(), 0);
    request["authority"]["frontend_sha"] = json!("b".repeat(40));
    configure(&mut inputs, &request, &generation, "api", &mut other).unwrap();
    assert_eq!(other.get_envs().count(), 0);
    request["authority"]["frontend_sha"] = json!(crate::recovery::frontend_build::b0::HEAD);
    drop(inputs);
    fs::write(projected, b"modified\n").unwrap();
    assert!(verify_projected(&mut Inputs::new(&fixture.root), &request, &generation).is_err());
}

use super::*;
use crate::recovery::fixture_control::portable_path;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const TOKEN: &str = "synthetic-private-recovery-token";
const SCOPE: &str = "runtime-environment-case";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    execution: PathBuf,
    private: PathBuf,
    api: PathBuf,
    worker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("runtime-start-environment-")
            .tempdir_in(local)
            .unwrap();
        let root = portable_path(&temporary.path().canonicalize().unwrap());
        let execution = root.join("execution");
        let evidence = root.join(".local-tests/evidence");
        let private = evidence.join("environment.json");
        let api = execution.join("api");
        let worker = evidence.join("worker");
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
            private,
            api,
            worker,
        };
        fixture.api_toml(2, 12341);
        fixture.worker_toml(3, 12342);
        fixture.write_private(&fixture.api);
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

    fn request(&self) -> Value {
        json!({"authority":{"scope_id":SCOPE,"api_endpoint":"http://127.0.0.1:12341/readyz",
            "worker_endpoint":"http://127.0.0.1:12342/readyz","frontend_endpoint":"http://127.0.0.1:12343"},
            "roots":{"backend_execution":self.execution},"tools":{"coordinator_root":self.root}})
    }

    fn target(&self) -> Value {
        let document = document::read(&self.private, &self.root, "隔离测试环境").unwrap();
        json!({"fresh_target":{"environment":document::binding(&document)}})
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
    let target = fixture.target();
    let mut request = fixture.request();
    let mut inputs = Inputs::new(&fixture.root);
    let snapshot = capture(&mut inputs, &request, &target).unwrap();
    assert!(request.get("environment").is_none());
    assert_eq!(snapshot["document"], target["fresh_target"]["environment"]);
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
    verify(&mut inputs, &request, &target).unwrap();
    verify_launch(&mut inputs, &request).unwrap();

    fs::write(fixture.worker.join("extra.toml"), b"unselected = true\n").unwrap();
    assert!(verify(&mut inputs, &request, &target).is_err());
    fs::remove_file(fixture.worker.join("extra.toml")).unwrap();
    fs::write(fixture.api.join("app.dev.toml"), b"unselected = fals\n").unwrap();
    assert!(verify(&mut inputs, &request, &target).is_err());
    assert!(verify_launch(&mut inputs, &request).is_err());
    fs::remove_file(fixture.api.join("app.dev.toml")).unwrap();
    assert!(verify(&mut inputs, &request, &target).is_err());
}

#[test]
fn rejects_wrong_private_binding_outside_configuration_duplicate_id_and_bad_toml() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let mut wrong = fixture.target();
    wrong["fresh_target"]["environment"]["sha256"] = json!("0".repeat(64));
    assert!(capture(&mut Inputs::new(&fixture.root), &request, &wrong).is_err());

    let target = fixture.target();
    let mut wrong_scope = request.clone();
    wrong_scope["authority"]["scope_id"] = json!("other-scope");
    assert!(capture(&mut Inputs::new(&fixture.root), &wrong_scope, &target).is_err());

    fixture.worker_toml(2, 12342);
    let result = capture(&mut Inputs::new(&fixture.root), &request, &target).unwrap_err();
    assert!(result.to_string().contains("实例编号必须不同"));
    fixture.worker_toml(3, 12343);
    assert!(capture(&mut Inputs::new(&fixture.root), &request, &target).is_err());
    fixture.worker_toml(3, 12342);

    fs::write(fixture.api.join("app.test.toml"), format!("[{TOKEN}\n")).unwrap();
    let result = capture(&mut Inputs::new(&fixture.root), &request, &target).unwrap_err();
    assert!(!result.to_string().contains(TOKEN));
    fixture.api_toml(2, 12341);

    let outside = fixture.root.join("unregistered");
    fs::create_dir(&outside).unwrap();
    fixture.write_private(&outside);
    assert!(capture(&mut Inputs::new(&fixture.root), &request, &fixture.target()).is_err());
}

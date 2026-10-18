use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use super::{Fixture, descriptor, write_json};
use crate::{
    Result,
    cli::{MonitoringBindOptions, MonitoringPorts, MonitoringTools},
    recovery::monitoring::{bind, contract, execution::ProtectedInputs, staging, tools},
};

struct FakeVerifier {
    value: Value,
    reads: Cell<usize>,
    fail_read: Option<usize>,
    version_reads: Cell<usize>,
    fail_versions: Option<usize>,
}

impl bind::Verifier for FakeVerifier {
    fn credential(&self, path: &Path) -> Result<staging::Credential> {
        staging::read_credential_with_security(path, |_| Ok(json!({"platform": "fixture"})))
    }
    fn authority(&self, _backend: &Path, _runtime: &Path, _target: &Path) -> Result<Value> {
        let count = self.reads.get() + 1;
        self.reads.set(count);
        if self.fail_read == Some(count) {
            return Err("fixture live generation drift".into());
        }
        Ok(self.value.clone())
    }

    fn versions(&self, paths: &BTreeMap<String, PathBuf>) -> Result<BTreeMap<String, String>> {
        let count = self.version_reads.get() + 1;
        self.version_reads.set(count);
        if self.fail_versions == Some(count) {
            return Err("fixture staged tool version drift".into());
        }
        assert_eq!(paths.len(), 4);
        for path in paths.values() {
            assert!(path.is_file());
        }
        Ok(tools::TOOL_VERSIONS
            .map(|(name, version)| (name.to_owned(), version.to_owned()))
            .into())
    }
}

struct Setup {
    fixture: Fixture,
    options: MonitoringBindOptions,
    verifier: FakeVerifier,
}

impl Setup {
    fn new() -> Self {
        let fixture = Fixture::new("fixed-alerts");
        fs::remove_file(&fixture.binding).unwrap();
        fs::remove_dir_all(fixture.run.join(contract::STAGING_DIRECTORY)).unwrap();
        for (_, relative) in staging::rules() {
            fs::copy(
                crate::workspace::root_dir().join(relative),
                fixture.backend.join(relative),
            )
            .unwrap();
        }
        fs::write(fixture.backend.join(".gitignore"), b".local-tests/\n").unwrap();
        for arguments in [
            vec!["init", "--quiet"],
            vec!["add", ".gitignore", "deploy"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.autocrlf=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(&fixture.backend)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let environment = fixture.backend.join(".local-tests/environment.json");
        write_json(
            &environment,
            &json!({"environment": {"APP_SCOPE_ID": "restore-scope", "APP_MONITOR_METRICS_BEARER_TOKEN": "secret"}}),
        );
        let mut authority = fixture.value["authority"].clone();
        authority["environment"] = descriptor(&environment);
        let directory = fixture.backend.join(".local-tests/tools");
        fs::create_dir(&directory).unwrap();
        let paths = tools::TOOL_VERSIONS.map(|(name, _)| {
            let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            fs::write(&path, name.as_bytes()).unwrap();
            path
        });
        let ports = free_ports();
        let options = MonitoringBindOptions {
            runtime_receipt: fixture.backend.join(".local-tests/runtime.json"),
            target_plan: fixture.backend.join(".local-tests/target.json"),
            output: fixture.binding.clone(),
            run_id: "restore-monitoring-24".into(),
            metrics_token_file: fixture.run.join(contract::TOKEN_NAME),
            tools: MonitoringTools {
                prometheus: paths[0].clone(),
                promtool: paths[1].clone(),
                alertmanager: paths[2].clone(),
                amtool: paths[3].clone(),
            },
            ports: MonitoringPorts {
                prometheus: ports[0],
                alertmanager: ports[1],
                webhook: ports[2],
            },
        };
        Self {
            fixture,
            options,
            verifier: FakeVerifier {
                value: authority,
                reads: Cell::new(0),
                fail_read: None,
                version_reads: Cell::new(0),
                fail_versions: None,
            },
        }
    }

    fn bind(&self) -> Result<Value> {
        bind::run_with(&self.options, &self.fixture.backend, &self.verifier)
    }
}

fn free_ports() -> [u16; 3] {
    let listeners = [0; 3].map(|_| TcpListener::bind("127.0.0.1:0").unwrap());
    listeners.map(|listener| listener.local_addr().unwrap().port())
}

#[test]
fn binding_stages_exact_inputs_and_rechecks_authority_before_publish_without_secret() {
    let setup = Setup::new();
    let value = setup.bind().unwrap();
    assert_eq!(setup.verifier.reads.get(), 2);
    assert_eq!(setup.verifier.version_reads.get(), 2);
    assert_eq!(value["status"], "bound");
    assert!(setup.options.output.is_file());
    assert!(
        !fs::read_to_string(&setup.options.output)
            .unwrap()
            .contains("secret")
    );
    assert_eq!(value["allowed_writes"].as_array().unwrap().len(), 5);
    assert_eq!(
        contract::validate_binding(&value_without_summary(&value)).unwrap()["run_id"],
        setup.options.run_id
    );
    assert!(setup.bind().is_err());
}

fn value_without_summary(value: &Value) -> Value {
    let mut value = value.clone();
    value.as_object_mut().unwrap().remove("_binding");
    value
}

#[test]
fn final_authority_or_staged_version_failure_never_publishes_binding() {
    for fail_authority in [false, true] {
        let mut setup = Setup::new();
        if fail_authority {
            setup.verifier.fail_read = Some(2);
        } else {
            setup.verifier.fail_versions = Some(2);
        }
        assert!(setup.bind().is_err());
        assert!(!setup.options.output.exists());
        assert!(setup.fixture.run.join(contract::STAGING_DIRECTORY).is_dir());
        assert!(setup.bind().is_err());
    }
}

#[test]
fn current_verification_uses_staged_tools_and_rejects_unknown_members_or_same_byte_replacement() {
    let setup = Setup::new();
    let value = value_without_summary(&setup.bind().unwrap());
    fs::write(&setup.options.tools.prometheus, b"source changed").unwrap();
    bind::verify_current_with(&setup.fixture.backend, &value, &setup.verifier).unwrap();
    for pointer in [
        "/credential/sha256",
        "/coordinator/executable/sha256",
        "/tools/prometheus/sha256",
        "/rules/alerts/sha256",
        "/authority/runtime/sha256",
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("0".repeat(64));
        assert!(
            bind::verify_current_with(&setup.fixture.backend, &changed, &setup.verifier).is_err(),
            "{pointer}"
        );
    }
    let staging_root = setup.fixture.run.join(contract::STAGING_DIRECTORY);
    fs::write(staging_root.join("unknown"), b"unknown").unwrap();
    assert!(bind::verify_current_with(&setup.fixture.backend, &value, &setup.verifier).is_err());
    fs::remove_file(staging_root.join("unknown")).unwrap();
    let path = PathBuf::from(value["tools"]["prometheus"]["path"].as_str().unwrap());
    let raw = fs::read(&path).unwrap();
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, raw).unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(bind::verify_current_with(&setup.fixture.backend, &value, &setup.verifier).is_err());
}

#[test]
fn credential_and_execution_guard_reject_drift_and_unknown_binding_fields() {
    let setup = Setup::new();
    let value = value_without_summary(&setup.bind().unwrap());
    let guard = ProtectedInputs::open([setup.options.metrics_token_file.clone()]).unwrap();
    guard.assert_unchanged().unwrap();
    #[cfg(windows)]
    assert!(fs::write(&setup.options.metrics_token_file, b"changed").is_err());
    drop(guard);
    fs::write(&setup.options.metrics_token_file, b"changed").unwrap();
    let error =
        bind::verify_current_with(&setup.fixture.backend, &value, &setup.verifier).unwrap_err();
    assert!(!error.to_string().contains("secret"));
    let mut unknown = value;
    unknown["unknown"] = json!(true);
    assert!(contract::validate_binding(&unknown).is_err());
}

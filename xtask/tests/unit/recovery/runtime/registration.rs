use std::{
    cell::{Cell, RefCell},
    process::Command,
};

use super::*;
use crate::recovery::runtime::REGISTRATION_LOCK;

struct Fixture {
    _workspace: tempfile::TempDir,
    backend: PathBuf,
    control: PathBuf,
    runtime: PathBuf,
    output: PathBuf,
    reference: PathBuf,
    target: PathBuf,
    configuration: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let workspace = tempfile::Builder::new()
            .prefix("runtime-registration-")
            .tempdir_in(local)
            .unwrap();
        let backend = portable_path(&workspace.path().canonicalize().unwrap());
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&backend)
                .status()
                .unwrap()
                .success()
        );
        fs::write(backend.join(".gitignore"), "/.local-tests/\n").unwrap();
        let control = backend.join(".local-tests");
        let dir = control.join("case");
        fs::create_dir_all(&dir).unwrap();
        let receipts = control.join("receipts");
        fs::create_dir(&receipts).unwrap();
        let reference = dir.join("reference.json");
        let target = dir.join("target.json");
        let configuration = dir.join("config");
        fs::create_dir(&configuration).unwrap();
        fs::write(
            configuration.join("app.test.toml"),
            b"scope_id = \"fixture\"\nsnowflake_worker_id = 3\n[monitor]\nmetrics_bearer_token = \"fixture-token\"\n",
        )
        .unwrap();
        fs::write(&reference, b"{\"reference\":true}\n").unwrap();
        fs::write(&target, b"{\"target\":true}\n").unwrap();
        Self {
            _workspace: workspace,
            backend,
            control,
            runtime: dir.join("runtime"),
            output: receipts.join("registration.json"),
            reference,
            target,
            configuration,
        }
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let (directory, runtime) = runtime_observation(&self.backend, &self.runtime)?;
        let reference = document::read(&self.reference, &self.backend, "参考计划夹具")?;
        let target = document::read(&self.target, &self.backend, "目标计划夹具")?;
        use crate::recovery::clone::native::target_verify::input::request_configuration::{
            configuration_files, configuration_identity,
        };
        let environment = std::collections::BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let (configuration, files) = configuration_files(
            &self.configuration,
            &environment,
            Some(&json!(self.configuration)),
        )?;
        let sha256 = configuration_identity(&configuration, &files)?;
        Ok(Snapshot {
            directory,
            configuration: configuration_registration::Source {
                directory: configuration,
                sha256,
            },
            receipt: json!({
                "format_version":2,"kind":"restore-runtime-registration",
                "reference_plan":document::binding(&reference),
                "target_plan":document::binding(&target),
                "observation":{"runtime":runtime,"ports_idle":ROLES},
                "remote_writes":0,
            }),
        })
    }
}

#[test]
fn register_rechecks_three_times_with_the_original_owner_and_never_overwrites() {
    let fixture = Fixture::new();
    let expected = fixture.snapshot().unwrap();
    let calls = Cell::new(0);
    let owner = fixture.control.join(REGISTRATION_LOCK).join("owner.json");
    let reply = register(&fixture.backend, &fixture.output, || {
        let n = calls.replace(calls.get() + 1);
        assert_eq!(fixture.output.exists(), n == 2);
        if n == 0 {
            assert!(!owner.exists());
        } else {
            let lock: Value = serde_json::from_slice(&fs::read(&owner)?).unwrap();
            let sha = expected.receipt["target_plan"]["sha256"].as_str().unwrap();
            assert_eq!(lock["operation"], format!("register:{sha}"));
        }
        fixture.snapshot()
    })
    .unwrap();
    assert_eq!(calls.get(), 3);
    assert!(!owner.exists());
    assert!(!fixture.runtime.exists());
    assert_eq!(reply["status"], "runtime_registered_not_started");
    let saved = document::read(&fixture.output, &fixture.backend, "新登记").unwrap();
    let mut observed = saved.value.clone();
    let private = observed
        .as_object_mut()
        .unwrap()
        .remove("runtime_environment")
        .unwrap();
    assert_eq!(observed, expected.receipt);
    configuration_registration::verify(
        &fixture.backend,
        &fixture.output,
        &private,
        &expected.configuration,
    )
    .unwrap();
    assert!(!saved.value.to_string().contains("fixture-token"));
    assert_eq!(reply["registration"], document::binding(&saved));
    assert_eq!(saved.value["observation"]["runtime"]["exists"], false);
    assert_eq!(
        saved.value["observation"]["runtime"]["anchor"]["path"],
        json!(fixture.runtime.parent().unwrap())
    );
    assert!(register(&fixture.backend, &fixture.output, || fixture.snapshot()).is_err());
    assert_eq!(fs::read(&fixture.output).unwrap(), saved.raw);
}

#[test]
fn register_rejects_output_inside_empty_runtime_before_taking_the_lock() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.runtime).unwrap();
    let observed = fixture.snapshot().unwrap();
    assert_eq!(observed.receipt["observation"]["runtime"]["exists"], true);
    let output = fixture.runtime.join("registration.json");
    let error = register(&fixture.backend, &output, || fixture.snapshot()).unwrap_err();
    assert!(error.to_string().contains("不能写入"));
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
    assert!(!output.exists());
}

#[test]
fn register_preserves_the_published_receipt_after_a_late_observation_fails() {
    let fixture = Fixture::new();
    let before = fixture.snapshot().unwrap();
    let error = register(&fixture.backend, &fixture.output, || {
        if fixture.output.is_file() {
            return Err("端口观察失败".into());
        }
        fixture.snapshot()
    })
    .unwrap_err();
    assert!(error.to_string().contains("保留登记文件"));
    assert!(error.to_string().contains("端口观察失败"));
    let mut saved = document::read(&fixture.output, &fixture.backend, "失败后保留的登记")
        .unwrap()
        .value;
    let private = saved
        .as_object_mut()
        .unwrap()
        .remove("runtime_environment")
        .unwrap();
    assert_eq!(saved, before.receipt);
    configuration_registration::verify(
        &fixture.backend,
        &fixture.output,
        &private,
        &before.configuration,
    )
    .unwrap();
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

#[test]
fn runtime_observation_uses_nearest_existing_anchor_and_refuses_unknown_entries() {
    let fixture = Fixture::new();
    let missing = fixture.runtime.join("nested/run");
    let (_, before) = runtime_observation(&fixture.backend, &missing).unwrap();
    assert_eq!(before["exists"], false);
    assert_eq!(
        before["anchor"]["path"],
        json!(fixture.runtime.parent().unwrap())
    );
    fs::create_dir(&fixture.runtime).unwrap();
    let (_, after) = runtime_observation(&fixture.backend, &missing).unwrap();
    assert_eq!(after["exists"], false);
    assert_eq!(after["anchor"]["path"], json!(fixture.runtime));
    assert_ne!(before["anchor"], after["anchor"]);
    fs::write(fixture.runtime.join("lifecycle.json"), b"{}").unwrap();
    assert!(runtime_observation(&fixture.backend, &fixture.runtime).is_err());
}

#[test]
fn zero_process_observation_checks_every_role_and_rejects_a_busy_port() {
    let endpoints = json!({
        "api":"http://127.0.0.1:18080/readyz",
        "worker":"http://[::1]:18081/readyz",
        "frontend":"http://127.0.0.1:18082",
    });
    let checked = RefCell::new(Vec::new());
    assert_eq!(
        closed_roles(&endpoints, |url| {
            checked.borrow_mut().push(url.to_owned());
            Ok(())
        })
        .unwrap(),
        ROLES
    );
    assert_eq!(
        checked.into_inner(),
        ROLES.map(|role| endpoints[role].as_str().unwrap())
    );
    assert!(
        closed_roles(&endpoints, |url| {
            if url == endpoints["worker"].as_str().unwrap() {
                return Err("端口已监听".into());
            }
            Ok(())
        })
        .is_err()
    );
}

#[test]
fn register_rejects_a_changed_anchor_after_taking_the_lock() {
    let fixture = Fixture::new();
    let calls = Cell::new(0);
    let result = register(&fixture.backend, &fixture.output, || {
        let n = calls.replace(calls.get() + 1);
        if n == 1 {
            fs::create_dir(&fixture.runtime)?;
        }
        fixture.snapshot()
    });
    assert!(result.is_err());
    assert_eq!(calls.get(), 2);
    assert!(!fixture.output.exists());
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

#[test]
fn directory_time_preserves_signed_and_unsigned_json_integers() {
    for expected in [i64::MIN, -1, 0, 1, i64::MAX] {
        assert_eq!(
            time_number(i128::from(expected)).unwrap().as_i64(),
            Some(expected)
        );
    }
    for expected in [i64::MAX as u64 + 1, u64::MAX] {
        assert_eq!(
            time_number(i128::from(expected)).unwrap().as_u64(),
            Some(expected)
        );
    }
    assert!(time_number(i128::from(i64::MIN) - 1).is_err());
    assert!(time_number(i128::from(u64::MAX) + 1).is_err());
}

#[test]
fn register_rejects_using_the_missing_runtime_path_as_output() {
    let fixture = Fixture::new();
    let error = register(&fixture.backend, &fixture.runtime, || fixture.snapshot()).unwrap_err();
    assert!(error.to_string().contains("不能写入"));
    assert!(!fixture.runtime.exists());
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

#[cfg(windows)]
#[test]
fn register_rejects_windows_case_aliases_inside_empty_or_missing_runtime() {
    let fixture = Fixture::new();
    let alias = fixture.runtime.with_file_name("RUNTIME");
    let error = register(&fixture.backend, &alias, || fixture.snapshot()).unwrap_err();
    assert!(error.to_string().contains("不能写入"));
    assert!(!fixture.runtime.exists());
    fs::create_dir(&fixture.runtime).unwrap();
    let output = alias.join("registration.json");
    let error = register(&fixture.backend, &output, || fixture.snapshot()).unwrap_err();
    assert!(error.to_string().contains("不能写入"));
    assert!(!output.exists());
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
}

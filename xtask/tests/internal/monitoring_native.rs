use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    recovery::monitoring::{contract, receipts, status},
    workspace::root_dir,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[path = "monitoring_binding_execution.rs"]
mod binding_execution;

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
    binding: PathBuf,
    value: Value,
}

impl Fixture {
    fn new(_alert: &str) -> Self {
        let backend = root_dir().join(".local-tests").join(format!(
            "mn-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let run = backend.join(".local-tests").join("monitor-run");
        let staging = run.join(contract::STAGING_DIRECTORY);
        fs::create_dir_all(staging.join("tools")).unwrap();
        fs::create_dir_all(staging.join("python")).unwrap();
        fs::create_dir_all(staging.join("tools/python")).unwrap();
        fs::write(run.join(contract::TOKEN_NAME), b"secret\n").unwrap();
        fs::write(staging.join("manifest.json"), b"{}\n").unwrap();
        for name in [
            "runtime.json",
            "target.json",
            "launch.json",
            "environment.json",
            "maintenance-build.json",
            "backend-build.json",
            "frontend-build.json",
            "source-generation.json",
            "lineage.json",
        ] {
            fs::write(backend.join(".local-tests").join(name), b"{}\n").unwrap();
        }
        let rules = backend.join("deploy").join("prometheus");
        fs::create_dir_all(&rules).unwrap();
        let alerts = rules.join("ryframe-alerts.yml");
        let tests = rules.join("ryframe-alerts.test.yml");
        let alerts_text = format!(
            "groups:\n  - name: fixture\n    rules:\n{}",
            contract::MONITORING_ALERTS
                .iter()
                .map(|alert| format!("      - alert: {alert}\n"))
                .collect::<String>()
        );
        fs::write(&alerts, alerts_text).unwrap();
        fs::write(
            &tests,
            contract::MONITORING_ALERTS
                .iter()
                .map(|alert| format!("alertname: {alert}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let value = binding(&backend, &run, &staging, &alerts, &tests);
        let binding = run.join("binding.json");
        write_json(&binding, &value);
        Self {
            backend,
            run,
            binding,
            value,
        }
    }

    fn replace_binding(&self) {
        write_json(&self.binding, &self.value);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.backend.exists()
            && let Err(error) = fs::remove_dir_all(&self.backend)
        {
            if std::thread::panicking() {
                eprintln!(
                    "监控测试失败现场清理失败，保留 {}：{error}",
                    self.backend.display()
                );
            } else {
                panic!("监控测试目录清理失败：{error}");
            }
        }
    }
}

#[test]
fn native_binding_rejects_bad_ports_endpoints_and_token_references() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    assert_eq!(
        contract::validate_binding(&fixture.value).unwrap(),
        fixture.value
    );
    let mut invalid_port = fixture.value.clone();
    invalid_port["ports"]["webhook"] = json!(80);
    assert!(contract::validate_binding(&invalid_port).is_err());
    let mut duplicate_port = fixture.value.clone();
    duplicate_port["ports"]["webhook"] = json!(29091);
    assert!(contract::validate_binding(&duplicate_port).is_err());
    let mut remote = fixture.value.clone();
    remote["endpoints"]["webhook"] = json!("http://example.com:29092");
    assert!(contract::validate_binding(&remote).is_err());
    let mut token = fixture.value.clone();
    token["credential"]["path"] = json!(fixture.run.join("other-token.txt").display().to_string());
    assert!(contract::validate_binding(&token).is_err());
}

#[test]
fn alert_and_evidence_mismatches_fail_closed() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    write_stage_files(&fixture);
    fixture.replace_binding();
    let mut result = passed_result(&fixture, &["RyFrameFixtureAlert"]);
    assert!(
        receipts::validate_result(&result, &fixture.value, &descriptor(&fixture.binding)).is_ok()
    );
    result["alerts"] = json!(["DifferentAlert"]);
    assert!(
        receipts::validate_result(&result, &fixture.value, &descriptor(&fixture.binding)).is_err()
    );

    write_json(
        &fixture.run.join("result.json"),
        &passed_result(&fixture, &["RyFrameFixtureAlert"]),
    );
    fs::write(fixture.run.join("start.json"), b"changed\n").unwrap();
    assert!(status::status(&fixture.backend, &fixture.binding).is_err());
}

#[test]
fn read_only_status_for_bound_run_writes_nothing() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    let before = directory_snapshot(&fixture.run);
    let binding_before = fs::read(&fixture.binding).unwrap();
    let native = status::status(&fixture.backend, &fixture.binding).unwrap();
    assert_eq!(native["status"], "bound");
    assert_eq!(native["next_action"], "start");
    assert_eq!(native["external_contacts"], 0);
    assert_eq!(directory_snapshot(&fixture.run), before);
    assert_eq!(fs::read(&fixture.binding).unwrap(), binding_before);

    assert_eq!(native["run_id"], "restore-monitoring-24");
    assert_eq!(native["scope_id"], "restore-scope");
    assert!(native["processes_alive"].as_object().unwrap().is_empty());
}

#[test]
fn passed_status_validates_exact_receipts_and_unknown_or_conflicting_receipts_fail() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    write_stage_files(&fixture);
    fixture.replace_binding();
    let result = passed_result(&fixture, &["RyFrameFixtureAlert"]);
    write_json(&fixture.run.join("result.json"), &result);
    let status = status::status(&fixture.backend, &fixture.binding).unwrap();
    assert_eq!(status["status"], "passed");
    assert_eq!(status["next_action"], Value::Null);

    fs::write(fixture.run.join("unknown.txt"), b"unknown").unwrap();
    assert!(contract::validate_root_shape(&fixture.run).is_err());
    fs::remove_file(fixture.run.join("unknown.txt")).unwrap();
    fs::write(fixture.run.join("observation-failure.json"), b"{}\n").unwrap();
    assert!(contract::validate_root_shape(&fixture.run).is_err());
    fs::remove_file(fixture.run.join("observation-failure.json")).unwrap();
    fs::write(
        fixture
            .run
            .join(".result.json.0123456789abcdef0123456789abcdef.pending"),
        b"{}\n",
    )
    .unwrap();
    assert!(status::status(&fixture.backend, &fixture.binding).is_err());
}

#[test]
fn binding_path_outside_owned_directory_and_links_are_rejected() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    let outside = root_dir().join(format!(
        "monitoring-outside-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    write_json(&outside, &fixture.value);
    assert!(contract::read_binding(&fixture.backend, &outside).is_err());
    fs::remove_file(&outside).unwrap();

    let linked_run = fixture.run.join("linked-run");
    fs::create_dir(&linked_run).unwrap();
    fs::write(linked_run.join(contract::TOKEN_NAME), b"secret\n").unwrap();
    fs::create_dir(linked_run.join(contract::STAGING_DIRECTORY)).unwrap();
    let link = linked_run.join("binding.json");
    let created = create_file_link(&fixture.binding, &link);
    if created {
        assert!(contract::read_binding(&fixture.backend, &link).is_err());
    }
}

#[test]
fn native_result_publishes_once_after_complete_evidence_and_then_status_reads_passed() {
    let fixture = Fixture::new("RyFrameFixtureAlert");
    prepare_complete_run(&fixture);
    let result = status::result(&fixture.backend, &fixture.binding).unwrap();
    assert_eq!(result["status"], "passed");
    let mut expected_alerts = contract::MONITORING_ALERTS.to_vec();
    expected_alerts.sort_unstable();
    assert_eq!(result["alerts"], json!(expected_alerts));
    let status = status::status(&fixture.backend, &fixture.binding).unwrap();
    assert_eq!(status["status"], "passed");
    assert_eq!(status["next_action"], Value::Null);
    assert!(status::result(&fixture.backend, &fixture.binding).is_err());
}

fn binding(backend: &Path, run: &Path, staging: &Path, alerts: &Path, tests: &Path) -> Value {
    let receipt = |name: &str| descriptor(&backend.join(".local-tests").join(name));
    let tool_version = |name: &str| match name {
        "prometheus" => "prometheus, version 3.5.0",
        "promtool" => "promtool, version 3.5.0",
        "alertmanager" => "alertmanager, version 0.34.0",
        _ => "amtool, version 0.34.0",
    };
    let tool = |name: &str| {
        json!({
            "path": staging.join("tools").join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
            "bytes": 2,
            "sha256": "b".repeat(64),
            "device": 1,
            "inode": 2,
            "version": tool_version(name),
        })
    };
    let authority = authority(backend);
    json!({
        "format_version": 1,
        "kind": "restore-monitoring-binding",
        "status": "bound",
        "run_id": "restore-monitoring-24",
        "scope_id": "restore-scope",
        "authority": authority,
        "coordinator": {
            "root": backend,
            "head": "a".repeat(40),
            "inventory_sha256": "c".repeat(64),
            "executable": descriptor(&std::env::current_exe().unwrap()),
        },
        "staging": descriptor(&staging.join("manifest.json")),
        "environment": receipt("environment.json"),
        "credential": {
            "path": run.join(contract::TOKEN_NAME),
            "bytes": 7,
            "sha256": "1".repeat(64),
            "device": 1,
            "inode": 5,
            "security_sha256": "2".repeat(64),
        },
        "tools": {
            "prometheus": tool("prometheus"),
            "promtool": tool("promtool"),
            "alertmanager": tool("alertmanager"),
            "amtool": tool("amtool"),
        },
        "rules": {
            "alerts": descriptor(alerts),
            "tests": descriptor(tests),
        },
        "endpoints": {
            "prometheus": "http://127.0.0.1:29090",
            "alertmanager": "http://127.0.0.1:29091",
            "webhook": "http://127.0.0.1:29092",
        },
        "ports": {"prometheus": 29090, "alertmanager": 29091, "webhook": 29092},
        "contact_policy": {
            "loopback_only": true,
            "send_resolved": true,
            "external_receivers": [],
        },
        "allowed_writes": ["alertmanager-data", "configs", "evidence", "processes", "prometheus-data"],
        "created_at": "2026-09-12T08:00:00Z",
    })
}

fn authority(backend: &Path) -> Value {
    let receipt = |name: &str| descriptor(&backend.join(".local-tests").join(name));
    json!({
        "format_version": 1,
        "kind": "restore-monitoring-authority",
        "runtime": receipt("runtime.json"),
        "target_plan": receipt("target.json"),
        "launch": receipt("launch.json"),
        "environment": receipt("environment.json"),
        "restore": {
            "id": "restore-one",
            "backup_id": "backup-one",
            "plan_hash": "4".repeat(64),
            "scope_id": "restore-scope",
            "data_verified_at": "2026-09-12T08:00:00Z",
        },
        "source": {
            "backup_source_sha": "1".repeat(40),
            "backend_product_sha": "2".repeat(40),
            "backend_execution_sha": "2".repeat(40),
            "backend_adapter_contract": null,
            "frontend_sha": "3".repeat(40),
        },
        "endpoints": {
            "api_ready": "http://127.0.0.1:18080/readyz",
            "worker_ready": "http://127.0.0.1:19091/readyz",
            "frontend": "http://127.0.0.1:14174",
            "api_metrics": "http://127.0.0.1:18080/api/v1/monitor/metrics",
            "worker_metrics": "http://127.0.0.1:19091/metrics",
        },
        "maintenance_execution": {
            "root": backend,
            "binding": {"kind": "current-backend", "path": backend},
            "build": receipt("maintenance-build.json"),
        },
        "product_execution": {
            "roots": {
                "source_backend": backend,
                "execution_backend": backend,
                "frontend": backend,
            },
            "backend_product_sha": "2".repeat(40),
            "backend_execution_sha": "2".repeat(40),
            "frontend_sha": "3".repeat(40),
            "builds": {
                "backend": receipt("backend-build.json"),
                "frontend": receipt("frontend-build.json"),
            },
            "adapter": null,
        },
        "processes": {
            "api": {"pid": 10, "started": "100", "executable": backend.join("api.exe")},
            "worker": {"pid": 11, "started": "101", "executable": backend.join("worker.exe")},
            "frontend": {"pid": 12, "started": "102", "executable": backend.join("frontend.exe")},
        },
        "source_generation": receipt("source-generation.json"),
        "dataset_lineage": receipt("lineage.json"),
    })
}

fn write_stage_files(fixture: &Fixture) {
    for name in [
        "start-intent.json",
        "start.json",
        "observation.json",
        "close-intent.json",
        "close.json",
    ] {
        fs::write(fixture.run.join(name), b"{}\n").unwrap();
    }
}

#[path = "monitoring_native_run.rs"]
mod run_fixture;
use run_fixture::prepare_complete_run;

fn passed_result(fixture: &Fixture, _alerts: &[&str]) -> Value {
    let mut alerts = contract::MONITORING_ALERTS.to_vec();
    alerts.sort_unstable();
    json!({
        "format_version": 1,
        "kind": "restore-monitoring-result",
        "status": "passed",
        "binding": descriptor(&fixture.binding),
        "start": descriptor(&fixture.run.join("start.json")),
        "observation": descriptor(&fixture.run.join("observation.json")),
        "close": descriptor(&fixture.run.join("close.json")),
        "run_id": "restore-monitoring-24",
        "scope_id": "restore-scope",
        "alerts": alerts,
        "targets": ["ryframe-api", "ryframe-worker"],
        "firing_delivered": true,
        "resolved_delivered": true,
        "boundary_cases": ["23h", "23h30s", "24h30s"],
        "ports_closed": ["alertmanager", "prometheus", "webhook"],
        "external_contacts": 0,
        "completed_at": "2026-09-12T08:00:00Z",
    })
}

fn descriptor(path: &Path) -> Value {
    let raw = fs::read(path).unwrap_or_default();
    json!({
        "path": path,
        "bytes": raw.len(),
        "sha256": hex::encode(Sha256::digest(&raw)),
    })
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn directory_snapshot(path: &Path) -> BTreeSet<String> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn create_file_link(source: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(source, target) {
            Ok(()) => true,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                ) =>
            {
                false
            }
            Err(error) => panic!("无法创建链接测试夹具：{error}"),
        }
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target).unwrap();
        true
    }
}

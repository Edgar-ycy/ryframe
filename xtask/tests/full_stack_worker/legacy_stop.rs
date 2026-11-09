use super::*;
use std::{io::Read, net::TcpStream, process::Child};

struct LegacyChild(Child);

impl LegacyChild {
    fn start(fixture: &Fixture, role: &str, port: u16) -> Self {
        let runtime = read(fixture.run.join("runtime.json"));
        let config = runtime[format!("{role}_config_dir")].as_str().unwrap();
        let binary = runtime["artifacts"][role]["path"].as_str().unwrap();
        let mut command = Command::new(binary);
        command
            .current_dir(&fixture.backend)
            .env_clear()
            .env("APP_ENV", "test")
            .args(["--config-dir", config])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for key in ["SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "PATH", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = Self(command.spawn().unwrap());
        wait(|| {
            if let Some(status) = child.0.try_wait().unwrap() {
                let mut diagnostic = String::new();
                child
                    .0
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut diagnostic)
                    .unwrap();
                panic!("{role} 替身在监听前退出（{status}）：{diagnostic}");
            }
            TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        write(
            fixture.run.join(format!("{role}.json")),
            &json!({"format_version":1,"role":role,"scope_id":"worker-native-tests",
                "identity":test_identity(child.0.id())}),
        );
        child
    }

    fn exited(&mut self) {
        wait(|| self.0.try_wait().unwrap().is_some());
    }
}

impl Drop for LegacyChild {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn stop(fixture: &Fixture, run: &Path, side: &str, roles: &[&str]) -> Output {
    clone_runtime(&fixture.backend, run, side, "stop")
        .arg("--roles")
        .args(roles)
        .arg("--write")
        .output()
        .unwrap()
}

#[test]
fn clone_runtime_stop_preserves_legacy_role_order_and_rejects_reused_identity() {
    let fixture = Fixture::with_role("normal", Role::Api);
    fs::write(fixture.backend.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir(fixture.backend.join("xtask")).unwrap();
    let local = fixture.backend.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let source = clone_status_run(&local, &fixture.run, "source", fixture.api_port);
    let target = clone_status_run(&local, &fixture.run, "target", fixture.api_port);
    for run in [&source, &target] {
        fs::create_dir(run.join("results")).unwrap();
    }
    let mut api = LegacyChild::start(&fixture, "api", fixture.api_port);
    let mut worker = LegacyChild::start(&fixture, "worker", fixture.port);
    let path = fixture.run.join("api.json");
    let original = fs::read(&path).unwrap();
    let mut bad = read(&path);
    bad["identity"]["started"] = json!("0");
    write(&path, &bad);

    rejected_status(
        stop(&fixture, &source, "source", &["api", "worker"]),
        "阶段 runtime-source 未完成",
    );
    assert!(api.0.try_wait().unwrap().is_none());
    assert!(worker.0.try_wait().unwrap().is_none());
    assert!(!fixture.run.join("producer-history.json").exists());
    assert!(!fixture.run.join("worker-control.lock").exists());
    fs::write(&path, original).unwrap();

    let source_summary = status_result(stop(&fixture, &source, "source", &["api", "worker"]));
    assert_eq!(
        source_summary,
        json!({"status":"stage_finished","stage":"runtime-source",
        "mode":"stop","attempt":2,"restore_qualified":false})
    );
    api.exited();
    worker.exited();
    let history = fixture.run.join("producer-history.json");
    let value = read(&history);
    let events = value["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    for (event, role) in events.iter().zip(["worker", "api"]) {
        assert_eq!(event["event"], "stopped");
        assert_eq!(event["role"], role);
        assert_eq!(
            event["identity"],
            read(fixture.run.join(format!("{role}.json")))["identity"]
        );
    }
    let saved = read(source.join("results").join("0002.json"));
    assert_eq!(saved["kind"], "devex-clone-runtime-control");
    assert_eq!(saved["producer_history"], json!(history));
    assert_eq!(
        saved["processes"]["api"],
        json!({"state":"stopped","identity":null,"ready":false})
    );
    assert_eq!(saved["processes"]["worker"], saved["processes"]["api"]);

    let mut worker = LegacyChild::start(&fixture, "worker", fixture.port);
    let target_summary = status_result(stop(&fixture, &target, "target", &["worker"]));
    assert_eq!(target_summary["stage"], "runtime-target");
    worker.exited();
    assert_eq!(read(&history)["events"].as_array().unwrap().len(), 3);
    assert!(!fixture.run.join("worker-control.lock").exists());
}

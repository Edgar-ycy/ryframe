use super::*;

#[test]
fn unconfirmed_parent_retains_false_and_requires_complete_linux_terminal_chain() {
    let (fixture, runtime) = failed_cleanup_fixture();
    let parent = fixture.output.join("failure.json");
    let mut value = document::read(&parent).unwrap();
    value["cleanup_completed"] = json!(false);
    document::write_replace(&parent, &value).unwrap();
    let bytes = std::fs::read(&parent).unwrap();
    std::fs::remove_file(fixture.output.join("failure-stop/stopped.json")).unwrap();
    let mut linux = linux_acceptance_failure(&fixture, &runtime);
    linux["diagnostic"] = super::super::diagnostic::Diagnostic {
        category: super::super::diagnostic::Category::AcknowledgementTimeout,
        write_outcome: super::super::diagnostic::Outcome::Stopped,
        os_code: None,
    }
    .value();
    fixture.write("linux-failure.json", linux.clone());
    let validate = || super::super::failed_cleanup::validate(&fixture.request, &fixture.output);
    let original = validate().unwrap();
    assert_eq!(original["runtime"], runtime);
    assert_eq!(original["reconcile_required"], true);
    for name in [
        "acknowledged.json",
        "accepted.json",
        "runtime.json",
        "failure-stop/stopped.json",
        "failure-stop/unknown.json",
    ] {
        fixture.write(name, json!({}));
        assert!(validate().is_err(), "{name}");
        std::fs::remove_file(fixture.output.join(name)).unwrap();
    }
    for name in [
        "linux-failure.json",
        "linux-exited.json",
        "linux-process.json",
    ] {
        let path = fixture.output.join(name);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(validate().is_err(), "missing {name}");
        std::fs::write(path, bytes).unwrap();
    }
    for (pointer, replacement) in [
        ("/cleanup_completed", json!(false)),
        ("/child_returncode", json!(9)),
        ("/identity/started", json!("different")),
        ("/diagnostic/write_outcome", json!("unknown")),
    ] {
        let mut changed = linux.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        document::write_replace(&fixture.output.join("linux-failure.json"), &changed).unwrap();
        assert!(validate().is_err());
    }
    document::write_replace(&fixture.output.join("linux-failure.json"), &linux).unwrap();
    assert_eq!(validate().unwrap(), original);
    assert_eq!(std::fs::read(&parent).unwrap(), bytes);
    let cleanup = super::super::cleanup_diagnostic::capture(&std::io::Error::from_raw_os_error(5));
    let sidecar = json!({"request_sha256":digest::plan_hash(&fixture.request).unwrap(),
        "launcher":runtime["launcher"],"cleanup":cleanup});
    fixture.write("cleanup-diagnostic.json", sidecar.clone());
    assert_eq!(validate().unwrap()["reconcile_required"], true);
    let mut changed = sidecar;
    changed["cleanup"]["operation"] = json!("private-marker");
    document::write_replace(&fixture.output.join("cleanup-diagnostic.json"), &changed).unwrap();
    assert!(
        !validate()
            .unwrap_err()
            .to_string()
            .contains("private-marker")
    );
    assert_eq!(std::fs::read(&parent).unwrap(), bytes);
    assert!(!fixture.output.join("runtime.json").exists());
    assert!(!fixture.output.join("failure-stop/stopped.json").exists());
}

fn linux_acceptance_failure(fixture: &Fixture, runtime: &Value) -> Value {
    use super::super::diagnostic::{Category, Diagnostic, Outcome};
    let mut intent = common::binding(&fixture.output.join("linux-intent.json")).unwrap();
    intent["path"] = json!(model::linux_path(intent["path"].as_str().unwrap()).unwrap());
    json!({"error_type":"NativeCacheFailure","diagnostic":Diagnostic {
        category:Category::AcceptanceTimeout,write_outcome:Outcome::Stopped,os_code:None
    }.value(),"cleanup_error_type":null,"identity":runtime["linux_identity"],
    "request_sha256":digest::plan_hash(&fixture.request).unwrap(),"cleanup_completed":true,
    "child_pid":runtime["linux_identity"]["pid"],"child_returncode":0,"popen_called":true,"intent":intent})
}

#[test]
fn linux_acceptance_failure_requires_the_same_complete_exit_chain_with_or_without_runtime() {
    for (published, returncode) in [(false, 0), (false, -15), (true, 0)] {
        let (fixture, runtime) = failed_cleanup_fixture();
        let ack = json!({"request_sha256":digest::plan_hash(&fixture.request).unwrap(),
            "identity":runtime["linux_identity"]});
        fixture.write("acknowledged.json", ack.clone());
        let mut failure = linux_acceptance_failure(&fixture, &runtime);
        failure["child_returncode"] = json!(returncode);
        document::write_replace(
            &fixture.output.join("linux-exited.json"),
            &json!({"identity":runtime["linux_identity"],"returncode":returncode}),
        )
        .unwrap();
        fixture.write("linux-failure.json", failure.clone());
        if published {
            fixture.write("accepted.json", ack);
            fixture.write("runtime.json", runtime.clone());
        }
        let validate = || super::super::failed_cleanup::validate(&fixture.request, &fixture.output);
        let proof = validate().unwrap();
        assert_eq!(proof["runtime"], runtime);
        assert_eq!(
            proof["bindings"]["redis_linux-failure.json"],
            common::binding(&fixture.output.join("linux-failure.json")).unwrap()
        );
        for name in [
            "acknowledged.json",
            "linux-exited.json",
            "failure-stop/stopped.json",
        ] {
            let path = fixture.output.join(name);
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert!(validate().is_err(), "missing {name}");
            std::fs::write(path, bytes).unwrap();
        }
        assert_eq!(validate().unwrap(), proof);
        assert_eq!(
            document::read(&fixture.output.join("linux-failure.json")).unwrap(),
            failure
        );
        assert_eq!(fixture.output.join("runtime.json").exists(), published);
    }
}

#[test]
fn linux_failure_rejects_incomplete_unknown_or_foreign_cleanup_and_fixed_diagnostic_drift() {
    let (fixture, runtime) = failed_cleanup_fixture();
    fixture.write(
        "acknowledged.json",
        json!({"request_sha256":digest::plan_hash(&fixture.request).unwrap(),
        "identity":runtime["linux_identity"]}),
    );
    let original = linux_acceptance_failure(&fixture, &runtime);
    fixture.write("linux-failure.json", original.clone());
    let path = fixture.output.join("linux-failure.json");
    let validate = || super::super::failed_cleanup::validate(&fixture.request, &fixture.output);
    let proof = validate().unwrap();
    for field in original.as_object().unwrap().keys() {
        let mut changed = original.clone();
        changed.as_object_mut().unwrap().remove(field);
        document::write_replace(&path, &changed).unwrap();
        assert!(validate().is_err(), "missing {field}");
    }
    for (pointer, value) in [
        ("/request_sha256", json!("different")),
        ("/intent/sha256", json!("different")),
        ("/identity/started", json!("different")),
        ("/child_pid", json!(999)),
        ("/child_returncode", json!(-15)),
        ("/cleanup_completed", json!(false)),
        ("/cleanup_error_type", json!("NativeCleanupUnknown")),
        ("/popen_called", json!(false)),
        ("/diagnostic/write_outcome", json!("unknown")),
        ("/diagnostic/category", json!("private-marker")),
        ("/diagnostic/advice", json!("private-marker")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        document::write_replace(&path, &changed).unwrap();
        let error = validate().unwrap_err().to_string();
        assert!(!error.contains("private-marker"));
    }
    let mut changed = original.clone();
    changed["unknown"] = json!(true);
    document::write_replace(&path, &changed).unwrap();
    assert!(validate().is_err());
    document::write_replace(&path, &original).unwrap();
    assert_eq!(validate().unwrap(), proof);
}

#[cfg(windows)]
#[test]
fn failed_cleanup_binds_real_stop_runtime_and_rejects_missing_tampered_or_unknown_evidence() {
    let (fixture, runtime) = failed_cleanup_fixture();
    let validate = || super::super::failed_cleanup::validate(&fixture.request, &fixture.output);
    let original = validate().unwrap();
    assert!(!original.to_string().contains("private-log-marker"));
    assert_eq!(original["runtime"], runtime);
    assert!(original["bindings"]["redis_failure-stop/stopped.json"].is_object());
    for name in [
        "request.json",
        "launch-request.json",
        "intent.json",
        "launcher.json",
        "linux-intent.json",
        "linux-process.json",
        "failure.json",
        "linux-exited.json",
        "failure-stop/stopped.json",
    ] {
        let path = fixture.output.join(name);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(validate().is_err(), "missing {name}");
        std::fs::write(&path, b"{}").unwrap();
        assert!(validate().is_err(), "tampered {name}");
        std::fs::write(&path, bytes).unwrap();
    }
    for (name, pointer, value) in [
        ("failure.json", "/cleanup_completed", json!(false)),
        ("failure.json", "/runtime_recorded", json!(false)),
        ("failure.json", "/launcher/started", json!("changed")),
        ("failure-stop/stopped.json", "/alive", json!(true)),
        (
            "failure-stop/stopped.json",
            "/resources_deleted",
            json!(true),
        ),
        (
            "failure-stop/stopped.json",
            "/runtime/linux_identity/started",
            json!("changed"),
        ),
        ("linux-exited.json", "/returncode", Value::Null),
        ("linux-exited.json", "/identity/started", json!("changed")),
    ] {
        let path = fixture.output.join(name);
        let bytes = std::fs::read(&path).unwrap();
        let mut changed = document::read(&path).unwrap();
        *changed.pointer_mut(pointer).unwrap() = value;
        document::write_replace(&path, &changed).unwrap();
        assert!(validate().is_err(), "{name} {pointer}");
        std::fs::write(path, bytes).unwrap();
    }
    for name in [
        "runtime.json",
        "partial.json",
        "unknown.json",
        "unknown.log",
        "failure-stop/unknown.json",
    ] {
        fixture.write(name, json!({}));
        assert!(validate().is_err(), "{name}");
        std::fs::remove_file(fixture.output.join(name)).unwrap();
    }
    assert_eq!(validate().unwrap(), original);
    assert!(!fixture.output.join("runtime.json").exists());
    assert!(!fixture.output.join("partial.json").exists());
}

#[cfg(windows)]
fn failed_cleanup_fixture() -> (Fixture, Value) {
    let fixture = Fixture::native();
    let runtime = fixture.recorded();
    for name in [
        "redis.stdout.log",
        "redis.stderr.log",
        "wsl.stdout.log",
        "wsl.stderr.log",
    ] {
        std::fs::write(fixture.output.join(name), b"private-log-marker").unwrap();
    }
    fixture.write("request.json", fixture.request.clone());
    fixture.write("failure.json", json!({"error_type":"NativeCacheFailure",
        "launcher":runtime["launcher"],"request_sha256":digest::plan_hash(&fixture.request).unwrap(),
        "runtime_recorded":true,"cleanup_completed":true}));
    std::fs::create_dir(fixture.output.join("failure-stop")).unwrap();
    fixture.write(
        "failure-stop/stopped.json",
        json!({"status":"redis_process_stopped","identity":runtime["linux_identity"],
        "boot_id":runtime["boot_id"],"terminated":true,
        "alive":false,"resources_deleted":false,"runtime":runtime}),
    );
    fixture.write(
        "linux-exited.json",
        json!({"identity":runtime["linux_identity"],"returncode":0}),
    );
    (fixture, runtime)
}

#[cfg(windows)]
#[test]
fn late_published_failure_requires_complete_acceptance_and_preserves_bound_evidence() {
    let (fixture, runtime) = failed_cleanup_fixture();
    let accepted = json!({"request_sha256":digest::plan_hash(&fixture.request).unwrap(),
        "identity":runtime["linux_identity"]});
    fixture.write("acknowledged.json", accepted.clone());
    fixture.write("accepted.json", accepted);
    fixture.write("runtime.json", runtime.clone());
    let validate = || super::super::failed_cleanup::validate(&fixture.request, &fixture.output);
    let original = validate().unwrap();
    assert_eq!(original["runtime"], runtime);
    assert_eq!(
        original["bindings"]["redis_runtime.json"],
        common::binding(&fixture.output.join("runtime.json")).unwrap()
    );
    for name in [
        "acknowledged.json",
        "accepted.json",
        "failure.json",
        "linux-exited.json",
        "failure-stop/stopped.json",
    ] {
        let path = fixture.output.join(name);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(validate().is_err(), "missing {name}");
        std::fs::write(path, bytes).unwrap();
    }
    for (name, pointer, value) in [
        ("acknowledged.json", "/request_sha256", json!("different")),
        ("accepted.json", "/identity/started", json!("different")),
        (
            "runtime.json",
            "/linux_identity/started",
            json!("different"),
        ),
        ("failure.json", "/cleanup_completed", json!(false)),
        (
            "failure-stop/stopped.json",
            "/runtime/boot_id",
            json!("different"),
        ),
        ("linux-exited.json", "/identity/started", json!("different")),
    ] {
        let path = fixture.output.join(name);
        let bytes = std::fs::read(&path).unwrap();
        let mut changed = document::read(&path).unwrap();
        *changed.pointer_mut(pointer).unwrap() = value;
        document::write_replace(&path, &changed).unwrap();
        assert!(validate().is_err(), "{name} {pointer}");
        std::fs::write(path, bytes).unwrap();
    }
    fixture.write("unknown.json", json!({}));
    assert!(validate().is_err());
    std::fs::remove_file(fixture.output.join("unknown.json")).unwrap();
    assert_eq!(validate().unwrap(), original);
    // 相同JSON内容的字节漂移也必须改变绑定，供正式部分关闭前后像复核拒绝。
    let path = fixture.output.join("runtime.json");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(b' ');
    std::fs::write(&path, &bytes).unwrap();
    let changed = validate().unwrap();
    assert_eq!(changed["runtime"], runtime);
    assert_ne!(
        changed["bindings"]["redis_runtime.json"],
        original["bindings"]["redis_runtime.json"]
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

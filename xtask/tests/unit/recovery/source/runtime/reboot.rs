use super::*;

#[test]
fn reboot_requires_all_original_members_and_strict_boot_margin_without_pid_substitution() {
    let boot = 130_000_000_000_000_000_u64;
    let started = boot - 300 * 10_000_000 - 1;
    let executable = std::env::current_exe().unwrap();
    let mut tree = json!({});
    for (member, pid) in [("process", 11), ("supervisor", 12), ("monitor", 13)] {
        tree[member] = json!({"pid":pid,"started":started.to_string(),"executable":executable});
    }
    let original = tree.clone();
    let facts = members(&tree, boot).unwrap();
    assert!(facts.values().all(|v| v["exited"] == true));
    assert_eq!(facts["monitor"]["identity"], original["monitor"]);
    tree["monitor"]["started"] = json!((started + 1).to_string());
    assert_eq!(members(&tree, boot).unwrap()["monitor"]["exited"], false);
    tree["monitor"] = original["monitor"].clone();
    tree["monitor"]["pid"] = tree["process"]["pid"].clone();
    assert!(members(&tree, boot).is_err());
    tree["monitor"] = Value::Null;
    assert!(members(&tree, boot).is_err());
    tree["monitor"] = original["monitor"].clone();
    tree["monitor"]["started"] = json!("unknown");
    assert!(members(&tree, boot).is_err());
}

#[test]
fn reboot_history_rejects_competing_control_and_later_source_stages() {
    let start = json!({"number":70,"stage":"seed-runtime","mode":"source-generation-start","status":"passed"});
    assert!(history(std::slice::from_ref(&start), &start).is_ok());
    for (stage, mode) in [
        ("runtime-target", "restart"),
        ("seed-runtime", "source-generation-stop"),
    ] {
        let row = json!({"number":71,"stage":stage,"mode":mode,"status":"running"});
        assert!(history(&[start.clone(), row], &start).is_err());
    }
    let close = json!({"number":71,"stage":"seed-runtime","mode":CLOSE,"status":"running"});
    assert!(history(&[start.clone(), close.clone()], &start).is_ok());
    assert!(history(&[start.clone(), close.clone(), close], &start).is_err());
    let later = json!({"number":71,"stage":"seed-runtime","mode":"source-generation-start","status":"passed"});
    assert!(history(&[start.clone(), later], &start).is_err());
}

struct ManagedRestart {
    backend: std::path::PathBuf,
    directory: tempfile::TempDir,
    start: Value,
    _owner: lock::RunLock,
}

impl ManagedRestart {
    fn new(stage: &str, mode: &str) -> Self {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let run = directory.path();
        document::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
        state::initialize(run).unwrap();
        // 完整连续账本；只验证受管门禁，不替代外层固定 C68/C69/C70 来源授权测试。
        let mut ledger = state::load(run, true).unwrap();
        ledger["attempts"] = json!(
            (1..70)
                .map(|number| json!({"number":number,
            "stage":"seed-runtime","mode":"source-generation-start","status":"failed",
            "started_at":"2026-10-28T00:00:00Z","finished_at":"2026-10-28T00:00:01Z",
            "sources":{},"result":null,"error_type":"ValueError"}))
                .collect::<Vec<_>>()
        );
        std::fs::write(run.join("state.json"), serde_json::to_vec(&ledger).unwrap()).unwrap();
        for name in ["request.json", "before.json", "running.json"] {
            document::write_new(&run.join(name), &json!({"fixture":name})).unwrap();
        }
        let request = common::binding(&run.join("request.json")).unwrap();
        let before = common::binding(&run.join("before.json")).unwrap();
        let running = common::binding(&run.join("running.json")).unwrap();
        let (number, _) =
            state::begin(run, "seed-runtime", "source-generation-start", &json!({})).unwrap();
        state::finish(
            run,
            number,
            Some(&json!({"request":request,"before":before,"running":running})),
            None,
        )
        .unwrap();
        let start = state::load(run, true).unwrap()["attempts"][69].clone();
        let (number, _) = state::begin(run, "seed-runtime", CLOSE, &json!({})).unwrap();
        Self::publish_closure(run, &start, &request, &before, &running);
        let result = document::read(&run.join("close-result.json")).unwrap();
        state::finish(run, number, Some(&result), None).unwrap();
        if stage == "cache-target" {
            let (number, _) = state::begin(run, "storage-target", "restart", &json!({})).unwrap();
            state::finish(
                run,
                number,
                Some(&json!({"fixture":"storage restart"})),
                None,
            )
            .unwrap();
        }
        let owner = lock::RunLock::claim(run).unwrap();
        let (number, attempt) =
            state::begin(run, stage, mode, &json!({"fixture":"current controller"})).unwrap();
        state::bind_controller(run, number, &owner.owner, &attempt).unwrap();
        Self {
            backend,
            directory,
            start,
            _owner: owner,
        }
    }

    fn publish_closure(
        run: &Path,
        start: &Value,
        request: &Value,
        before: &Value,
        running: &Value,
    ) {
        let directory = run.join("g0070/reboot-close");
        std::fs::create_dir_all(&directory).unwrap();
        let receipt = json!({"format_version":1,"kind":"seed-source-generation-reboot-closure",
            "status":"seed_source_generation_reboot_closed","start":start["result"],"request":request,
            "failed_prefix":{"producer":{}},"reboot":{"format_version":1,"kind":"host-boot-proof",
                "source":"GetTickCount64","margin_seconds":300,"observed_ticks":130000000000000000_u64,
                "uptime_ticks":36000000000_u64,"boot_ticks":129999964000000000_u64,"boot_time":"2012-12-14T22:06:40+00:00"},
            "roles":{},"storage":{},"cache":{"generations":[{"state":"stopped"}]},
            "baseline":before,"running":running,"declared_deltas":{"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0},
            "verification_qualified":false,"replay_allowed":false,"remote_writes":0,"restore_qualified":false});
        document::write_new(&directory.join("closure.json"), &receipt).unwrap();
        let mut result = receipt.clone();
        for key in [
            "format_version",
            "kind",
            "roles",
            "storage",
            "cache",
            "running",
        ] {
            result.as_object_mut().unwrap().remove(key);
        }
        result["producer"] = json!({});
        result["closure"] = common::binding(&directory.join("closure.json")).unwrap();
        document::write_new(&run.join("close-result.json"), &result).unwrap();
    }

    fn check(&self) -> Result<Value> {
        let ledger = state::load(self.directory.path(), true)?;
        current_restart(
            &self.backend,
            self.directory.path(),
            state::attempts(&ledger)?,
            &self.start,
        )
    }

    fn rewrite_owner(&self, change: impl FnOnce(&mut Value), both: bool) {
        let run = self.directory.path();
        let ledger = state::load(run, true).unwrap();
        let number = ledger["attempts"].as_array().unwrap().last().unwrap()["number"]
            .as_u64()
            .unwrap();
        let path = state::controller_path(run, number);
        let mut controller = document::read(&path).unwrap();
        change(&mut controller["owner"]);
        if both {
            std::fs::write(
                run.join("run.lock/owner.json"),
                serde_json::to_vec(&controller["owner"]).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(path, serde_json::to_vec(&controller).unwrap()).unwrap();
    }

    fn rewrite_closure(&self, key: &str, value: Value, mirror: bool) {
        let run = self.directory.path();
        let path = run.join("g0070/reboot-close/closure.json");
        let mut receipt = document::read(&path).unwrap();
        receipt[key] = value.clone();
        std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let result_path = state::result_path(run, 71);
        let mut result = document::read(&result_path).unwrap();
        result["closure"] = common::binding(&path).unwrap();
        if mirror {
            result[key] = value;
        }
        std::fs::write(&result_path, serde_json::to_vec(&result).unwrap()).unwrap();
        let mut ledger = state::load(run, false).unwrap();
        ledger["attempts"][70]["result"] = common::binding(&result_path).unwrap();
        std::fs::write(run.join("state.json"), serde_json::to_vec(&ledger).unwrap()).unwrap();
    }
}

#[test]
fn managed_restart_accepts_only_live_registered_storage_or_cache_controller_without_writes() {
    for stage in ["storage-target", "cache-target"] {
        let fixture = ManagedRestart::new(stage, "restart");
        let run = fixture.directory.path();
        let before = common::binding(&run.join("state.json")).unwrap();
        let owner = lock::owner_observation(run).unwrap();
        let first = fixture.check().unwrap();
        assert!(first["controller"]["controller"].is_object());
        assert_eq!(fixture.check().unwrap(), first);
        assert_eq!(common::binding(&run.join("state.json")).unwrap(), before);
        assert_eq!(lock::owner_observation(run).unwrap(), owner);
        // 收尾内保留的是历史公开 boot proof，不能被当前 1800 秒限制误拒绝。
    }
}

#[test]
fn managed_restart_rejects_wrong_stage_owner_reused_creation_and_unbound_controller() {
    for (stage, mode) in [("runtime-target", "restart"), ("storage-target", "start")] {
        assert!(ManagedRestart::new(stage, mode).check().is_err());
    }
    for (both, field) in [
        (false, "directory"),
        (true, "identity"),
        (true, "manifest_sha256"),
    ] {
        let fixture = ManagedRestart::new("storage-target", "restart");
        fixture.rewrite_owner(
            |owner| {
                if field == "identity" {
                    owner[field]["started"] = json!("1");
                } else {
                    owner[field] = json!("wrong");
                }
            },
            both,
        );
        assert!(fixture.check().is_err());
    }
    let fixture = ManagedRestart::new("storage-target", "restart");
    let run = fixture.directory.path();
    let path = state::controller_path(run, 72);
    let mut controller = document::read(&path).unwrap();
    controller["attempt_sha256"] = json!("0".repeat(64));
    std::fs::write(&path, serde_json::to_vec(&controller).unwrap()).unwrap();
    assert!(fixture.check().is_err());
    assert!(path.exists());
}

#[test]
fn managed_restart_rejects_forged_closure_even_after_all_file_bindings_are_recomputed() {
    for (field, value, mirror) in [
        ("start", json!({"forged":true}), true),
        ("request", json!({"forged":true}), true),
        ("replay_allowed", json!(true), false),
        ("remote_writes", json!(1), true),
        ("reboot", json!({"observed_ticks":1}), false),
    ] {
        let fixture = ManagedRestart::new("storage-target", "restart");
        fixture.rewrite_closure(field, value, mirror);
        assert!(fixture.check().is_err(), "{field}");
    }
    let fixture = ManagedRestart::new("storage-target", "restart");
    std::fs::remove_file(
        fixture
            .directory
            .path()
            .join("g0070/reboot-close/closure.json"),
    )
    .unwrap();
    assert!(fixture.check().is_err());
}

#[test]
fn managed_restart_rejects_nonlast_multiple_running_and_duplicate_passed_closure() {
    for multiple in [false, true] {
        let fixture = ManagedRestart::new("storage-target", "restart");
        let run = fixture.directory.path();
        let mut ledger = state::load(run, true).unwrap();
        let mut extra = ledger["attempts"][71].clone();
        extra["number"] = json!(73);
        if !multiple {
            extra["status"] = json!("failed");
            extra["finished_at"] = extra["started_at"].clone();
            extra["error_type"] = json!("ValueError");
        }
        ledger["attempts"].as_array_mut().unwrap().push(extra);
        std::fs::write(run.join("state.json"), serde_json::to_vec(&ledger).unwrap()).unwrap();
        let before = common::binding(&run.join("state.json")).unwrap();
        assert!(fixture.check().is_err());
        assert_eq!(common::binding(&run.join("state.json")).unwrap(), before);
    }
    let fixture = ManagedRestart::new("storage-target", "restart");
    let run = fixture.directory.path();
    let mut ledger = state::load(run, true).unwrap();
    let mut current = ledger["attempts"][71].clone();
    current["number"] = json!(73);
    let mut duplicate = ledger["attempts"][70].clone();
    duplicate["number"] = json!(72);
    let result = document::read(&state::result_path(run, 71)).unwrap();
    document::write_new(&state::result_path(run, 72), &result).unwrap();
    duplicate["result"] = common::binding(&state::result_path(run, 72)).unwrap();
    ledger["attempts"][71] = duplicate;
    ledger["attempts"].as_array_mut().unwrap().push(current);
    std::fs::write(run.join("state.json"), serde_json::to_vec(&ledger).unwrap()).unwrap();
    assert!(fixture.check().unwrap_err().to_string().contains("唯一"));
}

#[test]
fn native_reboot_protocol_rejects_injected_facts_before_reading_a_run() {
    for key in [
        "boot",
        "roles",
        "endpoints",
        "runtime",
        "start",
        "allow_attempt",
        "allow",
        "owner",
    ] {
        let mut args = json!({"backend":"missing", "directory":"missing"});
        args[key] = json!({});
        assert!(request(&args).unwrap_err().to_string().contains("字段"));
    }
    assert!(request(&json!({"backend":"missing","directory":"missing"})).is_err());
    assert!(require_closed(&json!({"blockers":["未知监听表"]})).is_err());
}

#[test]
fn reboot_failed_prefix_preserves_exact_original_producer_set_without_repair() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let run = tempfile::tempdir_in(parent).unwrap();
    let directory = run.path().join("verification");
    std::fs::create_dir(&directory).unwrap();
    let allowed = ready::verify(&json!({"action":"failed-entries","arguments":{}})).unwrap();
    for name in allowed.as_array().unwrap() {
        let name = name.as_str().unwrap();
        let path = directory.join(name);
        if ["before", "audit", "source-tools"].contains(&name) {
            std::fs::create_dir(path).unwrap();
        } else {
            std::fs::write(path, b"{}").unwrap();
        }
    }
    std::fs::write(directory.join("audit/login-audit.json"), b"{}").unwrap();
    std::fs::write(directory.join("before/image.json"), b"{}").unwrap();
    let failure = directory.join("failed.json");
    std::fs::write(&failure, br#"{"status":"failed","error":"ValueError"}"#).unwrap();
    let original = failed_prefix(&backend, &directory).unwrap();
    std::fs::write(directory.join("unknown.log"), b"unknown").unwrap();
    assert!(failed_prefix(&backend, &directory).is_err());
    assert!(directory.join("unknown.log").is_file());
    std::fs::remove_file(directory.join("unknown.log")).unwrap();
    let intent = directory.join("source-producer-intent.json");
    std::fs::remove_file(&intent).unwrap();
    assert!(failed_prefix(&backend, &directory).is_err());
    assert!(!intent.exists());
    std::fs::write(&intent, b"{}").unwrap();
    assert_eq!(failed_prefix(&backend, &directory).unwrap(), original);
    std::fs::write(
        &failure,
        br#"{"status":"failed","error":"CalledProcessError"}"#,
    )
    .unwrap();
    assert!(failed_prefix(&backend, &directory).is_err());
}

#[test]
fn original_start_bindings_reject_replaced_runtime_tree_identity_and_operation() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let output = tempfile::tempdir_in(parent).unwrap();
    let directory = output.path().join("runtime");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(output.path().join("intent.json"), b"{}").unwrap();
    std::fs::write(directory.join("runtime.json"), b"{}").unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut evidence = json!({"format_version":1,"kind":"seed-source-runtime-running",
        "intent":common::binding(&output.path().join("intent.json")).unwrap(),
        "runtime":common::binding(&directory.join("runtime.json")).unwrap(),"roles":{},"physical_binding":{}});
    let mut intent = json!({"operations":{}});
    for (index, role) in ["api", "worker"].into_iter().enumerate() {
        let pid = 100 + index * 3;
        let identity =
            |pid| json!({"pid":pid,"started":"130000000000000000","executable":executable});
        let operation = format!("{index:032x}");
        let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":directory,
            "role":role,"scope_id":"reboot-unit","operation_id":operation,"group_id":pid+1,
            "process":identity(pid),"supervisor":identity(pid+1),"monitor":identity(pid+2)});
        std::fs::write(
            directory.join(format!("{role}.json")),
            serde_json::to_vec(&json!({
            "format_version":1,"role":role,"scope_id":"reboot-unit","identity":identity(pid)}))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join(format!("{role}-tree.json")),
            serde_json::to_vec(&tree).unwrap(),
        )
        .unwrap();
        evidence["roles"][role] = json!({"ready":true,
            "process":common::binding(&directory.join(format!("{role}.json"))).unwrap(),
            "tree":common::binding(&directory.join(format!("{role}-tree.json"))).unwrap()});
        intent["operations"][role] = json!(operation);
    }
    let path = output.path().join("running-evidence.json");
    std::fs::write(&path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    let facts = json!({"receipt":{"runtime_evidence":common::binding(&path).unwrap(),"intent":evidence["intent"]}});
    assert_eq!(
        running_evidence(&backend, output.path(), &facts).unwrap(),
        evidence
    );
    let selected = json!({"scope_id":"reboot-unit"});
    assert!(roles::observe(&directory, &evidence, &intent, &selected, &json!({}), false).is_ok());
    intent["operations"]["api"] = json!("f".repeat(32));
    assert!(roles::observe(&directory, &evidence, &intent, &selected, &json!({}), false).is_err());
    intent["operations"]["api"] = json!("0".repeat(32));
    let tree = directory.join("api-tree.json");
    let mut replaced = document::read(&tree).unwrap();
    replaced["process"]["started"] = json!("130000000000000001");
    std::fs::write(&tree, serde_json::to_vec(&replaced).unwrap()).unwrap();
    assert!(roles::observe(&directory, &evidence, &intent, &selected, &json!({}), false).is_err());
    std::fs::write(directory.join("runtime.json"), b"{\"replaced\":true}").unwrap();
    assert!(running_evidence(&backend, output.path(), &facts).is_err());
}

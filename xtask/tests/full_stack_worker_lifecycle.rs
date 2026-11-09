//! API/Worker 的真实进程、取消与完整成员回收场景；不接触共享服务。
#[path = "support/process_liveness.rs"]
mod process_liveness;
#[path = "full_stack_worker/support.rs"]
mod support;
use serde_json::{Value, json};
use std::{fs, net::TcpListener};
use support::*;

#[test]
fn api_survives_controller_exit_and_stops_its_complete_role_tree() {
    let fixture = Fixture::with_role("tree", Role::Api);
    let runtime = read(fixture.run.join("runtime.json"));
    let started = fixture.ok("start-strict");
    assert_eq!(started["operation"], "start");
    let id = started["operation_id"].as_str().unwrap();
    let archive = fixture.archive(&started["operation_id"]);
    let owner = read(archive.join("owner.json"));
    let request = read(archive.join("request.json"));
    assert_eq!(owner["kind"], "full-stack-api-control-owner");
    assert_eq!(request["kind"], "full-stack-api-control-request");
    assert_eq!(started["kind"], "full-stack-api-control-result");
    for receipt in [&owner, &request] {
        assert_eq!(receipt["operation_id"], started["operation_id"]);
        assert_eq!(receipt["source_sha256"], started["source_sha256"]);
    }
    assert_eq!(owner["identity"], started["controller_identity"]);
    fixture.gone(owner["identity"]["pid"].as_u64().unwrap() as u32);
    assert_eq!(request["api_artifact"], runtime["artifacts"]["api"]);
    assert_eq!(
        request["api_ready_url"],
        format!("http://127.0.0.1:{}/readyz", fixture.api_port)
    );
    assert!(request.get("worker_artifact").is_none());
    assert!(request.get("worker_ready_url").is_none());
    assert_eq!(request["log"], format!("api-{id}.log"));
    assert!(fixture.run.join(request["log"].as_str().unwrap()).is_file());
    assert!(!fixture.run.join("api-control.lock").exists());
    assert_eq!(
        started["identity"]["executable"],
        runtime["artifacts"]["api"]["path"]
    );
    assert_ne!(
        started["identity"]["executable"],
        runtime["artifacts"]["worker"]["path"]
    );
    let tree = fixture.tree();
    assert_eq!(tree["role"], "api");
    assert_eq!(tree["operation_id"], started["operation_id"]);
    assert_eq!(tree["process"], started["identity"]);
    assert_eq!(tree["group_id"], tree["supervisor"]["pid"]);
    for field in ["process", "supervisor", "monitor"] {
        assert!(process_liveness::process_is_running(
            tree[field]["pid"].as_u64().unwrap() as u32
        ));
    }
    let status = fixture.ok("status");
    assert_eq!(status["state"], "running");
    assert_eq!(status["identity"], started["identity"]);
    assert_eq!(fixture.tree(), tree);
    assert!(TcpListener::bind(("127.0.0.1", fixture.port)).is_ok());
    let descendant: u32 = fs::read_to_string(fixture.run.join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let stopped = fixture.ok("stop-strict");
    assert_eq!(stopped["operation"], "stop");
    assert_eq!(stopped["state"], "stopped");
    assert!(stopped["identity"].is_null());
    assert!(!fixture.control("start-strict", 5.0).0);
    assert_eq!(fixture.tree(), tree);
    assert_eq!(fixture.ok("status")["state"], "stopped");
    let proof_path = fixture.run.join(format!("api-members-{id}-stopped.json"));
    let proof_bytes = fs::read(&proof_path).unwrap();
    let verified = fixture.ok("stop-strict");
    assert_eq!(verified["state"], "stopped");
    assert!(verified["identity"].is_null());
    assert_eq!(fs::read(&proof_path).unwrap(), proof_bytes);
    let proof = read(proof_path);
    assert_eq!(proof["role"], "api");
    assert_eq!(proof["scope_id"], runtime["scope_id"]);
    assert_eq!(proof["supervisor"], tree["supervisor"]);
    assert_eq!(proof["monitor"], tree["monitor"]);
    assert_eq!(proof["status"], "stopped");
    assert!(proof["error_type"].is_null());
    let members = proof["members"].as_array().unwrap();
    assert!(members.contains(&tree["supervisor"]));
    assert!(members.contains(&tree["process"]));
    assert!(
        members
            .iter()
            .any(|identity| identity["pid"].as_u64() == Some(u64::from(descendant)))
    );
    for field in ["process", "supervisor", "monitor"] {
        fixture.gone(tree[field]["pid"].as_u64().unwrap() as u32);
    }
    fixture.gone(descendant);
    assert!(TcpListener::bind(("127.0.0.1", fixture.api_port)).is_ok());
    for name in [
        "worker-started",
        "worker.json",
        "worker-tree.json",
        "worker-control-receipts",
    ] {
        assert!(!fixture.run.join(name).exists());
    }
}

#[test]
fn start_persists_atomic_receipts_survives_controller_and_is_idempotently_stopped() {
    let fixture = Fixture::new("ready");
    let result = fixture.ok("start");
    let identity = &result["identity"];
    assert!(process_liveness::process_is_running(
        identity["pid"].as_u64().unwrap() as u32
    ));
    assert!(!fixture.run.join("worker-control.lock").exists());
    let archive = fixture.archive(&result["operation_id"]);
    let mut names = fs::read_dir(&archive)
        .unwrap()
        .map(|v| v.unwrap().file_name().to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [
            "candidate.json",
            "owner.json",
            "progress.json",
            "request.json",
            "result.json",
            "supervisor.json"
        ]
    );
    for name in ["owner.json", "request.json", "result.json"] {
        let value = read(archive.join(name));
        assert_eq!(value["operation_id"], result["operation_id"]);
        assert_eq!(value["source_sha256"], result["source_sha256"]);
    }
    assert!(!fixture.control("start", 1.0).0);
    assert_eq!(fixture.ok("status")["identity"], *identity);
    assert_eq!(fixture.ok("reconcile")["identity"], *identity);
    let tree = fixture.tree();
    fixture.ok("stop");
    fixture.ok("stop");
    assert_eq!(fixture.ok("status")["state"], "stopped");
    for field in ["process", "supervisor", "monitor"] {
        fixture.gone(tree[field]["pid"].as_u64().unwrap() as u32);
    }
    let id = tree["operation_id"].as_str().unwrap();
    let proof = read(
        fixture
            .run
            .join(format!("worker-members-{id}-stopped.json")),
    );
    assert_eq!(proof["status"], "stopped");
    assert!(
        proof["members"]
            .as_array()
            .unwrap()
            .contains(&tree["process"])
    );
}
#[test]
fn timeout_is_archived_only_after_complete_member_cleanup_and_allows_restart() {
    let fixture = Fixture::new("idle");
    let (ok, _) = fixture.control("start", 0.2);
    assert!(!ok);
    assert!(!fixture.run.join("worker-control.lock").exists());
    let entries = fs::read_dir(fixture.run.join("worker-control-receipts"))
        .unwrap()
        .map(|v| v.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    let result = read(entries[0].join("result.json"));
    assert_eq!(result["outcome"], "failed");
    assert!(result["error"].as_str().unwrap().contains("就绪超时"));
    let tree = fixture.tree();
    for field in ["process", "supervisor", "monitor"] {
        fixture.gone(tree[field]["pid"].as_u64().unwrap() as u32);
    }
    fs::write(fixture.run.join("mode"), "ready").unwrap();
    fixture.ok("start");
    fixture.ok("stop");
}
#[test]
fn early_exit_and_natural_parent_exit_reclaim_descendants_before_archiving() {
    for mode in ["exit", "tree-exit"] {
        let fixture = Fixture::new(mode);
        assert!(!fixture.control("start", 2.0).0);
        assert!(!fixture.run.join("worker-control.lock").exists());
        if mode == "tree-exit" {
            let pid = fs::read_to_string(fixture.run.join("descendant.pid"))
                .unwrap()
                .parse()
                .unwrap();
            fixture.gone(pid);
        }
        fs::write(fixture.run.join("mode"), "ready").unwrap();
        fixture.ok("start");
        fixture.ok("stop");
    }
}
#[test]
fn stop_and_crash_reclaim_complete_descendant_tree_and_record_requested_mode() {
    for action in ["stop", "crash"] {
        let fixture = Fixture::new("tree");
        fixture.ok("start");
        let tree = fixture.tree();
        let descendant = fs::read_to_string(fixture.run.join("descendant.pid"))
            .unwrap()
            .parse()
            .unwrap();
        fixture.ok(action);
        fixture.gone(descendant);
        let id = tree["operation_id"].as_str().unwrap();
        assert_eq!(
            read(fixture.run.join(format!("worker-tree-{id}-control.json")))["mode"],
            if action == "crash" { "crash" } else { "normal" }
        );
        assert_eq!(
            read(fixture.run.join(format!("worker-tree-{id}-result.json")))["termination"],
            if cfg!(windows) || action == "crash" {
                "forced"
            } else {
                "normal"
            }
        );
    }
}
#[test]
fn parent_control_death_during_readiness_does_not_orphan_or_replay_worker() {
    let fixture = Fixture::new("delay");
    let mut controller = fixture.command("start", 5.0);
    fixture.wait_started(&mut controller);
    controller.kill().unwrap();
    controller.wait().unwrap();
    let lock = fixture.run.join("worker-control.lock");
    wait(|| lock.join("result.json").exists());
    assert_eq!(read(lock.join("result.json"))["state"], "running");
    let worker = fixture.ok("status");
    assert_eq!(worker["state"], "running");
    fixture.ok("stop");
}
#[test]
fn parent_control_death_preserves_failure_and_waits_for_complete_cleanup() {
    let fixture = Fixture::new("idle");
    let mut controller = fixture.command("start", 0.3);
    wait(|| fixture.run.join("worker-started").exists());
    controller.kill().unwrap();
    controller.wait().unwrap();
    let lock = fixture.run.join("worker-control.lock");
    wait(|| lock.join("result.json").exists());
    let failure = read(lock.join("result.json"));
    assert_eq!(failure["outcome"], "failed");
    assert_eq!(fixture.ok("status")["state"], "stopped");
    assert_eq!(
        read(
            fixture
                .archive(&failure["operation_id"])
                .join("result.json")
        ),
        failure
    );
}
#[test]
fn interrupted_prepared_start_and_simple_operations_reconcile_to_exact_goal() {
    let fixture = Fixture::new("ready");
    let first = prepared(&fixture, "start");
    assert_eq!(fixture.ok("reconcile")["state"], "stopped");
    let failure = read(fixture.archive(&first["operation_id"]).join("result.json"));
    assert_eq!(failure["outcome"], "failed");
    assert_eq!(failure["reconciled"], true);
    fixture.ok("start");
    for action in ["status", "reconcile"] {
        prepared(&fixture, action);
        assert_eq!(fixture.ok("status")["state"], "running");
    }
    let stop = prepared(&fixture, "stop");
    assert_eq!(fixture.ok("status")["state"], "stopped");
    assert_eq!(
        read(fixture.archive(&stop["operation_id"]).join("result.json"))["reconciled"],
        true
    );
}
fn prepared(fixture: &Fixture, action: &str) -> Value {
    // 模拟已经死亡的控制器在原子 claim 后中断，不修改任何产品数据。
    let source = hash(
        &json!({"runtime":{"bytes":fs::metadata(fixture.run.join("runtime.json")).unwrap().len(),"sha256":sha(&fs::read(fixture.run.join("runtime.json")).unwrap())},"runtime_evidence":null}),
    );
    let id = uuid::Uuid::new_v4().simple().to_string();
    let common = json!({"format_version":1,"kind":"full-stack-worker-control-owner","operation_id":id,"runtime_directory":fixture.run,"source_sha256":source});
    let mut owner = common.clone();
    owner["identity"] = json!({"pid":2147481001u32,"started":"1","executable":fixture.binary});
    owner["created_at_ns"] = json!(1);
    let runtime = read(fixture.run.join("runtime.json"));
    let mut request = common;
    request["kind"] = json!("full-stack-worker-control-request");
    for (key,value) in json!({"operation":action,"backend_root":fixture.backend,"scope_id":runtime["scope_id"],"controller_identity":owner["identity"],"owner_sha256":hash(&owner),"runtime_receipt_sha256":sha(&fs::read(fixture.run.join("runtime.json")).unwrap()),"worker_artifact":runtime["artifacts"]["worker"],"worker_ready_url":runtime["worker_ready_url"],"timeout_seconds":1,"log":if action=="start"{json!(format!("worker-{id}.log"))}else{Value::Null},"requested_at_ns":2}).as_object().unwrap(){request[key]=value.clone();}
    let lock = fixture.run.join("worker-control.lock");
    fs::create_dir(&lock).unwrap();
    write(lock.join("owner.json"), &owner);
    write(lock.join("request.json"), &request);
    owner
}
#[test]
fn duplicate_unknown_and_tampered_control_receipts_fail_closed_without_releasing_lock() {
    for mutation in ["duplicate", "unknown", "result", "child"] {
        let fixture = Fixture::new("ready");
        prepared(&fixture, "start");
        let lock = fixture.run.join("worker-control.lock");
        match mutation {
            "duplicate" => {
                let owner = fs::read_to_string(lock.join("owner.json")).unwrap();
                fs::write(
                    lock.join("owner.json"),
                    owner.replacen('{', "{\"format_version\":1,", 1),
                )
                .unwrap();
            }
            "unknown" => write(lock.join("foreign.json"), &json!({})),
            "result" => write(lock.join("result.json"), &json!({"outcome":"succeeded"})),
            _ => write(
                lock.join("candidate.json"),
                &json!({"identity":{"pid":std::process::id()}}),
            ),
        }
        assert!(!fixture.control("reconcile", 1.0).0);
        assert!(lock.exists());
        assert!(!fixture.run.join("worker-started").exists());
    }
}
#[test]
fn empty_legacy_lock_requires_explicit_reconcile_and_never_accepts_unknown_contents() {
    let fixture = Fixture::new("ready");
    fs::create_dir(fixture.run.join("worker-control.lock")).unwrap();
    assert!(!fixture.control("status", 1.0).0);
    assert!(!fixture.control("start", 1.0).0);
    assert_eq!(fixture.ok("reconcile")["state"], "stopped");
    assert!(
        fs::read_dir(fixture.run.join("worker-control-receipts"))
            .unwrap()
            .any(|v| v
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("legacy-"))
    );
}
#[test]
fn config_binary_and_source_drift_are_rejected_before_claim_or_process_control() {
    for mutation in ["configuration", "artifact", "source"] {
        let fixture = Fixture::new("ready");
        match mutation {
            "configuration" => fs::write(
                fixture.run.join("worker-config/app.test.toml"),
                "private-secret-invalid",
            )
            .unwrap(),
            "artifact" => fs::write(&fixture.binary, b"private-secret-invalid").unwrap(),
            _ => write(
                fixture.run.join("runtime-evidence.json"),
                &json!({"secret":"private-secret-invalid"}),
            ),
        };
        let (ok, value) = fixture.control("start", 1.0);
        assert!(!ok);
        assert!(!value.to_string().contains("private-secret-invalid"));
        assert!(!fixture.run.join("worker-control.lock").exists());
        assert!(!fixture.run.join("worker-started").exists());
    }
}
#[test]
fn occupied_port_and_foreign_ready_listener_cannot_be_adopted_as_worker() {
    let fixture = Fixture::new("ready");
    let listener = TcpListener::bind(("127.0.0.1", fixture.port)).unwrap();
    assert!(!fixture.control("start", 1.0).0);
    assert!(!fixture.run.join("worker-control.lock").exists());
    drop(listener);
    fixture.ok("start");
    let original = read(fixture.run.join("worker.json"));
    let mut wrong = original.clone();
    wrong["identity"]["started"] = json!("1");
    write(fixture.run.join("worker.json"), &wrong);
    assert!(!fixture.control("stop", 1.0).0);
    assert!(process_liveness::process_is_running(
        original["identity"]["pid"].as_u64().unwrap() as u32
    ));
    write(fixture.run.join("worker.json"), &original);
    let lock = fixture.run.join("worker-control.lock");
    fs::remove_file(lock.join("result.json")).unwrap();
    fixture.ok("reconcile");
    fixture.ok("stop");
}
#[test]
fn unready_registered_worker_and_reused_identity_are_never_reported_running() {
    let fixture = Fixture::new("ready");
    let result = fixture.ok("start");
    let original = read(fixture.run.join("worker.json"));
    let mut wrong = original.clone();
    wrong["identity"]["pid"] = json!(std::process::id());
    write(fixture.run.join("worker.json"), &wrong);
    assert!(!fixture.control("status", 1.0).0);
    write(fixture.run.join("worker.json"), &original);
    assert!(process_liveness::process_is_running(
        result["identity"]["pid"].as_u64().unwrap() as u32
    ));
    let lock = fixture.run.join("worker-control.lock");
    fs::remove_file(lock.join("result.json")).unwrap();
    fixture.ok("reconcile");
    fixture.ok("stop");
}
#[test]
fn unknown_boolean_timeout_and_control_credentials_are_rejected_without_lock() {
    let fixture = Fixture::new("ready");
    for (action, timeout) in [("unknown", 1.0), ("start", 0.0), ("start", 181.0)] {
        assert!(!fixture.control(action, timeout).0);
        assert!(!fixture.run.join("worker-control.lock").exists());
    }
}

#[test]
fn live_unready_worker_and_foreign_ready_listener_are_rejected_with_no_signal() {
    use std::{
        io::{Read, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let fixture = Fixture::new("ready");
    let started = fixture.ok("start");
    fs::write(fixture.run.join("disable-ready"), "").unwrap();
    wait(|| TcpListener::bind(("127.0.0.1", fixture.port)).is_ok());
    assert!(!fixture.control("status", 1.0).0);
    let listener = TcpListener::bind(("127.0.0.1", fixture.port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let server = std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                    let mut bytes = [0; 4096];
                    let _ = stream.read(&mut bytes);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(error) => panic!("{error}"),
            }
        }
    });
    assert!(!fixture.control("reconcile", 1.0).0);
    assert!(process_liveness::process_is_running(
        started["identity"]["pid"].as_u64().unwrap() as u32
    ));
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    fixture.ok("stop");
}

#[test]
fn running_configuration_drift_fails_before_control_and_preserves_live_worker() {
    let fixture = Fixture::new("ready");
    let started = fixture.ok("start");
    let path = fixture.run.join("worker-config/app.test.toml");
    let before = fs::read(&path).unwrap();
    fs::write(&path, "private-secret-invalid").unwrap();
    let (ok, value) = fixture.control("stop", 1.0);
    assert!(!ok);
    assert!(!value.to_string().contains("private-secret-invalid"));
    assert!(!fixture.run.join("worker-control.lock").exists());
    assert!(process_liveness::process_is_running(
        started["identity"]["pid"].as_u64().unwrap() as u32
    ));
    fs::write(&path, before).unwrap();
    fixture.ok("stop");
}

#[test]
fn supervisor_death_reclaims_worker_and_descendant_with_monitor_proof() {
    let fixture = Fixture::new("tree");
    fixture.ok("start");
    let tree = fixture.tree();
    let pid = tree["supervisor"]["pid"].as_u64().unwrap() as u32;
    #[cfg(windows)]
    {
        use winsafe::{FILETIME, HPROCESS, co, prelude::kernel_Hprocess};
        let process = HPROCESS::OpenProcess(
            co::PROCESS::TERMINATE
                | co::PROCESS::QUERY_LIMITED_INFORMATION
                | co::PROCESS::SYNCHRONIZE,
            false,
            pid,
        )
        .unwrap();
        let (mut creation, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        process
            .GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)
            .unwrap();
        assert_eq!(
            ((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
                .to_string(),
            tree["supervisor"]["started"].as_str().unwrap()
        );
        process.TerminateProcess(137).unwrap();
    }
    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };
        kill(Pid::from_raw(i32::try_from(pid).unwrap()), Signal::SIGKILL).unwrap();
    }
    let descendant = fs::read_to_string(fixture.run.join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    fixture.gone(descendant);
    for field in ["process", "supervisor", "monitor"] {
        fixture.gone(tree[field]["pid"].as_u64().unwrap() as u32);
    }
    assert_eq!(fixture.ok("status")["state"], "stopped");
    let id = tree["operation_id"].as_str().unwrap();
    assert_eq!(
        read(
            fixture
                .run
                .join(format!("worker-members-{id}-stopped.json"))
        )["status"],
        "stopped"
    );
}

#[cfg(windows)]
#[test]
fn status_reconciles_exact_worker_tree_terminated_with_its_outer_job() {
    use std::{
        io::{Seek, SeekFrom, Write},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let fixture = Fixture::new("tree");
    let mut input = tempfile::tempfile_in(&fixture.run).unwrap();
    let request = json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"control","arguments":{"action":"start","backend":fixture.backend,"directory":fixture.run,"timeout":5}});
    input
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    input.seek(SeekFrom::Start(0)).unwrap();
    let output = fixture.run.join("outer-start.json");
    let log = fs::File::create(&output).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("__full-stack-worker")
        .env("APP_ENV", "test")
        .stdin(input)
        .stdout(log)
        .stderr(Stdio::null());
    let group = processkit::ProcessGroup::new().unwrap();
    let mut controller = group.spawn(tokio::process::Command::from(command)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = controller.try_wait().unwrap() {
            assert!(status.success(), "{}", fs::read_to_string(&output).unwrap());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let tree = fixture.tree();
    drop(group);
    for field in ["process", "supervisor", "monitor"] {
        fixture.gone(tree[field]["pid"].as_u64().unwrap() as u32);
    }
    let result = fixture.ok("status");
    assert_eq!(result["state"], "stopped");
    assert_eq!(result["reconciled"], true);
    let id = tree["operation_id"].as_str().unwrap();
    let evidence = read(
        fixture
            .run
            .join(format!("worker-external-termination-{id}.json")),
    );
    assert_eq!(evidence["normal_shutdown_proof"], Value::Null);
    assert_eq!(evidence["status"], "external-termination");
}

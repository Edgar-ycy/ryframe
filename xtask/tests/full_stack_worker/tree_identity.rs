use super::{Fixture, Path, Value, fs, json, process_liveness::process_is_running, read, wait};

#[test]
fn wrong_worker_identity_leaves_both_descendants_for_original_tree_crash() {
    let fixture = Fixture::new("tree-deep");
    let started = fixture.ok("start");
    let tree = fixture.tree();
    assert_eq!(tree["operation_id"], started["operation_id"]);
    assert_eq!(tree["process"], started["identity"]);
    let product = u32::try_from(tree["process"]["pid"].as_u64().unwrap()).unwrap();
    let [descendant, grandchild] =
        ["descendant.pid", "grandchild.pid"].map(|name| read_pid(&fixture.run.join(name)));
    assert_ne!(product, descendant);
    assert_ne!(product, grandchild);
    assert_ne!(descendant, grandchild);

    let id = tree["operation_id"].as_str().unwrap();
    let running = fixture.run.join("worker.json");
    let tree_path = fixture.run.join("worker-tree.json");
    let control = fixture.run.join(format!("worker-tree-{id}-control.json"));
    let result = fixture.run.join(format!("worker-tree-{id}-result.json"));
    let closed = fixture
        .run
        .join(format!("worker-members-{id}-stopped.json"));
    let failed_path = fixture.run.join("worker-control.lock/result.json");
    let start_result = fixture.archive(&tree["operation_id"]).join("result.json");
    let start_bytes = fs::read(&start_result).unwrap();
    let tree_bytes = fs::read(&tree_path).unwrap();
    let running_bytes = fs::read(&running).unwrap();
    let mut wrong = read(&running);
    assert_eq!(wrong["identity"], tree["process"]);
    let original_started = wrong["identity"]["started"].as_str().unwrap();
    let wrong_started = if original_started == "1" { "2" } else { "1" };
    wrong["identity"]["started"] = json!(wrong_started);
    let tampered = serde_json::to_vec_pretty(&wrong).unwrap();

    for action in ["stop", "crash"] {
        fs::write(&running, &tampered).unwrap();
        let (ok, rejection) = fixture.control(action, 1.0);
        let received = fs::read(&running);
        fs::write(&running, &running_bytes).unwrap();
        assert!(!ok, "{rejection}");
        assert_eq!(rejection["error"], "WorkerRejected");
        assert_eq!(rejection["ok"], false);
        assert_eq!(received.unwrap(), tampered);
        assert!(!control.exists());
        assert!(!result.exists());
        assert!(!closed.exists());
        assert_eq!(fs::read(&tree_path).unwrap(), tree_bytes);
        assert_eq!(fs::read(&start_result).unwrap(), start_bytes);
        for pid in [product, descendant, grandchild] {
            assert!(process_is_running(pid), "{action} 影响了原进程 {pid}");
        }

        let failed_bytes = fs::read(&failed_path).unwrap();
        let failed = read(&failed_path);
        assert_eq!(failed["operation"], action);
        assert_eq!(failed["outcome"], "failed");
        assert_eq!(failed["state"], "unknown");
        assert!(failed["identity"].is_null());
        assert_eq!(fixture.ok("reconcile")["identity"], tree["process"]);
        assert_eq!(
            fs::read(fixture.archive(&failed["operation_id"]).join("result.json")).unwrap(),
            failed_bytes
        );
    }

    let crashed = fixture.ok("crash");
    assert_eq!(crashed["operation"], "crash");
    assert_eq!(crashed["state"], "stopped");
    assert!(crashed["identity"].is_null());
    for pid in [product, descendant, grandchild] {
        fixture.gone(pid);
    }
    assert_eq!(fs::read(&tree_path).unwrap(), tree_bytes);
    assert_eq!(fs::read(&start_result).unwrap(), start_bytes);
    assert_original_crash_receipts(
        &tree,
        [&control, &result, &closed],
        [descendant, grandchild],
    );
}

fn read_pid(marker: &Path) -> u32 {
    let mut pid = None;
    wait(|| {
        pid = fs::read_to_string(marker)
            .ok()
            .and_then(|text| text.parse().ok());
        pid.is_some()
    });
    pid.unwrap()
}

fn assert_original_crash_receipts(
    tree: &Value,
    [control, result, closed]: [&Path; 3],
    descendants: [u32; 2],
) {
    let id = &tree["operation_id"];
    let control = read(control);
    assert_eq!(&control["operation_id"], id);
    assert_eq!(control["mode"], "crash");
    let result = read(result);
    assert_eq!(&result["operation_id"], id);
    assert_eq!(result["process"], tree["process"]);
    assert_eq!(result["termination"], "forced");
    let proof = read(closed);
    assert_eq!(&proof["operation_id"], id);
    assert_eq!(proof["supervisor"], tree["supervisor"]);
    assert_eq!(proof["monitor"], tree["monitor"]);
    assert_eq!(proof["status"], "stopped");
    assert!(proof["error_type"].is_null());
    let members = proof["members"].as_array().unwrap();
    assert!(members.contains(&tree["process"]));
    for pid in descendants {
        assert!(members.iter().any(|member| {
            member["pid"].as_u64() == Some(u64::from(pid))
                && member["executable"] == tree["process"]["executable"]
        }));
    }
}

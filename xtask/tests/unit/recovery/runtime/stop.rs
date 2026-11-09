use super::*;

#[test]
fn every_role_is_attempted_in_reverse_and_original_failure_shape_is_preserved() {
    let mut generation = json!({
        "number":3, "directory":"registered-directory", "status":"stopping",
        "roles":{"api":{"state":"running"},"worker":{"state":"running"},"frontend":{"state":"running"}},
        "error_type":"PriorError"
    });
    let observed =
        json!({"api":"api evidence","worker":"worker evidence","frontend":"frontend evidence"});
    let mut attempted = Vec::new();
    let failures = stop_roles(&mut generation, &observed, |value, role, evidence| {
        attempted.push(role.to_owned());
        assert_eq!(evidence, &observed[role]);
        if role == "worker" {
            return Err("原关闭证明缺失".into());
        }
        value["roles"][role]["state"] = json!("stopped");
        Ok(())
    });
    assert_eq!(attempted, ["frontend", "worker", "api"]);
    assert_eq!(failures, ["worker：原关闭证明缺失"]);
    set_outcome(&mut generation, failures.is_empty());
    assert_eq!(generation["status"], "stop_failed");
    assert_eq!(generation["error_type"], "RuntimeStopError");
    assert_eq!(generation["roles"]["frontend"]["state"], "stopped");
    assert_eq!(generation["roles"]["worker"]["state"], "running");
    assert_eq!(generation["roles"]["api"]["state"], "stopped");

    set_outcome(&mut generation, true);
    assert_eq!(generation["status"], "stopped");
    assert_eq!(generation["error_type"], Value::Null);
    assert_eq!(
        reply("already_stopped", &generation, &observed),
        json!({
            "format_version":1, "kind":"restore-runtime-lifecycle-result", "operation":"stop",
            "status":"already_stopped", "generation":3, "runtime_directory":"registered-directory",
            "processes":observed,
        })
    );
}

#[test]
fn ready_without_original_completion_cannot_be_marked_stopped() {
    let backend = crate::workspace::root_dir();
    let root = backend.join(".local-tests/runtime-stop-tests");
    fs::create_dir_all(&root).unwrap();
    let generation_directory = tempfile::tempdir_in(root).unwrap();
    let directory = generation_directory.path();
    let operation = "1".repeat(32);
    let generation = json!({"roles":{"worker":{"operation_id":operation}}});
    let ready = directory.join(format!("worker-members-{operation}-ready.json"));
    let mut inputs = Inputs::new(&backend);
    pending::verify(&backend, directory, &generation, "worker", &mut inputs).unwrap();

    fs::write(&ready, b"{}").unwrap();
    let error =
        pending::verify(&backend, directory, &generation, "worker", &mut inputs).unwrap_err();
    assert!(error.to_string().contains("保留待定状态"));
    assert_eq!(fs::read(ready).unwrap(), b"{}");
    assert!(
        !directory
            .join(format!("worker-tree-{operation}-control.json"))
            .exists()
    );
}

use super::*;

#[test]
fn first_generation_follows_only_the_same_registered_start_through_recovery_chain() {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let control = super::super::registration::protocol_path(temporary.path()).unwrap();
    let allowed = Allowed {
        start: format!("runtime-start:{}:{}", "a".repeat(64), "b".repeat(64)),
        recover: format!("runtime-recover:{}:{}", "a".repeat(64), "b".repeat(64)),
        values: Vec::new(),
    };
    let first = owner(&control, &allowed.start, "1", 45123);
    let second = owner(&control, &allowed.recover, "2", 45124);
    let third = owner(&control, &allowed.recover, "3", 45125);
    archive(&control, &second, &first);
    archive(&control, &third, &second);
    assert!(allowed.first_start(&control, 1, &third).unwrap());
    assert!(!allowed.first_start(&control, 2, &third).unwrap());

    let foreign = owner(&control, "runtime-start:foreign", "4", 45126);
    archive(&control, &second, &foreign);
    assert!(!allowed.first_start(&control, 1, &third).unwrap());
    archive(&control, &second, &third);
    let error = allowed.first_start(&control, 1, &third).unwrap_err();
    assert!(error.to_string().contains("循环"));
}

fn owner(control: &Path, operation: &str, token: &str, pid: u32) -> Value {
    json!({
        "format_version":1,
        "kind":"restore-runtime-registration-controller",
        "runtime_directory":control,
        "operation":operation,
        "token":token.repeat(32),
        "identity":{"pid":pid,"started":"135000000000000000","executable":std::env::current_exe().unwrap()},
    })
}

fn archive(control: &Path, current: &Value, original: &Value) {
    let token = current["token"].as_str().unwrap();
    let path = control.join(format!("restore-registration-reconcile-{token}.json"));
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "format_version":1,
            "kind":"restore-runtime-registration-reconciliation",
            "owner":original,
            "observed_identity":null,
            "remote_writes_reconciled":false,
        }))
        .unwrap(),
    )
    .unwrap();
}

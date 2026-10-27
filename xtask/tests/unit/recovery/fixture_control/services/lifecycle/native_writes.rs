use super::*;

#[test]
fn native_controller_recovery_rejects_live_owner_without_writes() {
    let fixture = Fixture::new();
    let owner = RunLock::claim(&fixture.run).unwrap();
    let descriptor = binding(&fixture.run.join("run.lock/owner.json")).unwrap();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    let names = fs::read_dir(&fixture.run).unwrap().count();
    assert!(controller_recovery::run(&fixture.backend, &fixture.run, &descriptor).is_err());
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), before);
    assert_eq!(fs::read_dir(&fixture.run).unwrap().count(), names);
    owner.verify().unwrap();
    owner.release().unwrap();
}

#[test]
fn native_dead_controller_recovery_binds_preimage_and_interrupts_only_owned_attempt() {
    let fixture = Fixture::new();
    let _guard = ProcessGuard::acquire(&fixture.run).unwrap();
    let owner = RunLock::claim(&fixture.run).unwrap();
    let mut dead = owner.owner.clone();
    dead["identity"]["pid"] = json!(2147481000u32);
    let owner_file = fixture.run.join("run.lock/owner.json");
    json_io::write_replace(&owner_file, &dead).unwrap();
    let (number, attempt) =
        state::begin(&fixture.run, "fixture-services", "close", &json!({})).unwrap();
    state::bind_controller(&fixture.run, number, &dead, &attempt).unwrap();
    let descriptor = binding(&state::controller_path(&fixture.run, number)).unwrap();
    let before = binding(&fixture.run.join("state.json")).unwrap();
    let result = controller_recovery::run(&fixture.backend, &fixture.run, &descriptor).unwrap();
    assert_eq!(result["owner"], dead);
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["copy_requires_reconciliation"], true);
    assert!(!fixture.run.join("run.lock").exists());
    let saved = fixture.state();
    assert_eq!(saved["attempts"][3]["error_type"], "ControllerInterrupted");
    assert_eq!(saved["attempts"][3]["status"], "failed");
    assert!(
        saved["attempts"].as_array().unwrap()[..3]
            .iter()
            .all(|value| value["status"] == "passed")
    );
    let intent = fs::read_dir(&fixture.run)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("recovery-")
        })
        .unwrap();
    let intent = read(&intent).unwrap();
    assert_eq!(intent["owner"], descriptor);
    assert_eq!(intent["state_before"], before);
    assert_eq!(intent["state_after_sha256"], result["state_sha256"]);
    assert!(controller_recovery::run(&fixture.backend, &fixture.run, &descriptor).is_err());
}

#[test]
fn native_tree_stop_rejects_foreign_tree_and_reused_pid_before_control_publication() {
    let fixture = Fixture::new();
    let mut tree = fixture.services["tree"].clone();
    tree["kind"] = json!("full-stack-process-tree");
    tree["group_id"] = tree["supervisor"]["pid"].clone();
    let root = fixture.run.join("rustfs");
    json_io::write_replace(&root.join("rustfs-tree.json"), &tree).unwrap();
    let mut foreign = tree.clone();
    foreign["scope_id"] = json!("foreign-scope");
    assert!(tree_control::stop(&fixture.backend, &foreign, false).is_err());
    tree["supervisor"] = crate::recovery::clone::native::lifecycle::identity::current().unwrap();
    tree["group_id"] = tree["supervisor"]["pid"].clone();
    tree["supervisor"]["started"] = json!("1");
    json_io::write_replace(&root.join("rustfs-tree.json"), &tree).unwrap();
    assert!(tree_control::stop(&fixture.backend, &tree, true).is_err());
    assert!(
        !root
            .join(format!("rustfs-tree-{}-control.json", "b".repeat(32)))
            .exists()
    );
}

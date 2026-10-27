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

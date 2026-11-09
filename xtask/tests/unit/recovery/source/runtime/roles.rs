use super::*;
use std::fs;

#[test]
fn native_role_directory_keeps_unclaimed_prefix_readonly_but_rejects_orphaned_evidence() {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let runtime = temporary.path();
    let operations = json!({"api":"a".repeat(32),"worker":"b".repeat(32)});
    fs::write(runtime.join("runtime.json"), b"{}\n").unwrap();
    fs::write(runtime.join("binaries.json"), b"{}\n").unwrap();
    fs::write(runtime.join("api-control.guard"), [0]).unwrap();
    directory(runtime, &operations).unwrap();
    for name in [
        "api-tree.json".to_owned(),
        format!("api-members-{}-ready.json", "a".repeat(32)),
        format!("api-members-{}-stopped.json", "a".repeat(32)),
        format!("api-tree-{}-control.json", "a".repeat(32)),
        format!("api-{}.log", "a".repeat(32)),
        "unregistered.log".into(),
    ] {
        let path = runtime.join(name);
        fs::write(&path, b"{}\n").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(directory(runtime, &operations).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_file(path).unwrap();
    }
    fs::write(runtime.join("api-control.guard"), [1]).unwrap();
    assert!(directory(runtime, &operations).is_err());
    assert_eq!(fs::read(runtime.join("api-control.guard")).unwrap(), [1]);
}

#[test]
fn native_role_directory_rejects_unbound_lock_archive_and_hardlink_without_repair() {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let runtime = temporary.path();
    let operations = json!({"api":"a".repeat(32),"worker":"b".repeat(32)});
    let guard = runtime.join("api-control.guard");
    fs::write(&guard, [0]).unwrap();
    let lock = runtime.join("api-control.lock");
    fs::create_dir(&lock).unwrap();
    assert!(directory(runtime, &operations).is_err());
    assert!(lock.is_dir());
    fs::remove_dir(&lock).unwrap();
    let archives = runtime.join("api-control-receipts");
    fs::create_dir(&archives).unwrap();
    let foreign = archives.join("c".repeat(32));
    fs::create_dir(&foreign).unwrap();
    assert!(directory(runtime, &operations).is_err());
    assert!(foreign.is_dir());
    fs::remove_dir(&foreign).unwrap();
    fs::remove_dir(&archives).unwrap();
    let held = runtime.join("held");
    fs::hard_link(&guard, &held).unwrap();
    assert!(directory(runtime, &operations).is_err());
    assert_eq!(fs::read(&held).unwrap(), [0]);
}

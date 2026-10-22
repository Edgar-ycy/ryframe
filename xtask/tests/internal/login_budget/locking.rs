use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let local = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/login-budget-locks");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let path = directory.path().join("budget.json");
    (directory, path)
}

#[test]
fn occupied_node_lock_times_out_without_stealing_or_deleting_it() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let lock_path = paths::appended(&path, ".lock");
    fs::write(&lock_path, []).unwrap();
    let result = OwnedLock::acquire_for(&path, &parents, Duration::ZERO);
    assert!(matches!(result, Err(BudgetError::LockBusy)));
    assert_eq!(fs::read(lock_path).unwrap(), Vec::<u8>::new());
    assert!(!path.exists());
}

#[test]
fn owned_lock_has_a_token_is_exclusive_and_is_removed_on_drop() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let lock_path = paths::appended(&path, ".lock");
    {
        let lock = OwnedLock::acquire(&path, &parents).unwrap();
        lock.verify().unwrap();
        assert_eq!(fs::read(&lock_path).unwrap().len(), 36);
        assert!(paths::create_new(&lock_path).is_err());
    }
    assert!(!lock_path.exists());
}

#[test]
fn another_file_with_the_same_token_is_never_deleted() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let mut lock = OwnedLock::acquire(&path, &parents).unwrap();
    let token = fs::read(&lock.path).unwrap();
    fs::rename(&lock.path, paths::appended(&path, ".previous-lock")).unwrap();
    fs::write(&lock.path, &token).unwrap();
    assert_eq!(lock.verify(), Err(BudgetError::LockOwner));
    assert_eq!(lock.release(), Err(BudgetError::LockOwner));
    drop(lock);
    assert_eq!(fs::read(paths::appended(&path, ".lock")).unwrap(), token);
}

#[test]
fn token_changes_on_the_same_held_file_prevent_unlocking() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let mut lock = OwnedLock::acquire(&path, &parents).unwrap();
    fs::write(&lock.path, b"external-owner").unwrap();
    assert_eq!(lock.release(), Err(BudgetError::LockOwner));
    drop(lock);
    assert_eq!(
        fs::read(paths::appended(&path, ".lock")).unwrap(),
        b"external-owner"
    );
}

#[test]
fn cooperative_lock_release_can_be_observed_without_waiting_for_a_budget_window() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let lock_path = paths::appended(&path, ".lock");
    fs::write(&lock_path, []).unwrap();
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(75));
        fs::remove_file(lock_path).unwrap();
    });
    let mut lock = OwnedLock::acquire_for(&path, &parents, Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    lock.release().unwrap();
}

#[test]
fn non_file_lock_is_rejected_and_preserved() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let lock_path = paths::appended(&path, ".lock");
    fs::create_dir(&lock_path).unwrap();
    assert!(matches!(
        OwnedLock::acquire(&path, &parents),
        Err(BudgetError::File)
    ));
    assert!(lock_path.is_dir());
}

#[cfg(unix)]
#[test]
fn symbolic_lock_is_rejected_without_modifying_its_target() {
    let (_directory, path) = fixture();
    let parents = Parents::prepare(&path).unwrap();
    let target = paths::appended(&path, ".owner");
    fs::write(&target, b"external-owner").unwrap();
    std::os::unix::fs::symlink(&target, paths::appended(&path, ".lock")).unwrap();
    assert!(matches!(
        OwnedLock::acquire(&path, &parents),
        Err(BudgetError::File)
    ));
    assert_eq!(fs::read(target).unwrap(), b"external-owner");
}

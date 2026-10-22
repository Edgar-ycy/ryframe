use std::sync::{Arc, Barrier};

use super::*;

#[test]
fn constructor_does_not_create_files_and_first_reservation_publishes_the_node_shape() {
    let fixture = Fixture::new();
    let path = fixture.path.parent().unwrap().join("new/inner/budget.json");
    let ledger =
        LedgerFile::new(path.clone(), Binding::new("login-test", 5, 60_000).unwrap()).unwrap();
    assert!(!path.parent().unwrap().exists());
    assert!(!fixture.path.exists());
    ledger
        .try_reserve("system", "admin", "198.18.1.2", 1000)
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    let encoded = std::str::from_utf8(&bytes).unwrap();
    assert!(encoded.ends_with('\n'));
    assert!(encoded.contains(r#""binding":{"scope":"login-test","capacity":5,"windowMs":60000}"#));
    assert!(!encoded.contains("admin"));
    let state: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(state["version"], 1);
    assert_eq!(state["observedAt"], 1000);
    assert_eq!(state["buckets"].as_object().unwrap().len(), 2);
    assert!(!path.with_file_name("budget.json.lock").exists());
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn static_node_ledger_is_readable_and_rust_output_retains_node_compatible_binding_order() {
    let fixture = Fixture::new();
    fs::write(&fixture.path, include_bytes!("node-ledger.json")).unwrap();
    let ledger = fixture.ledger();
    fixture.attempt(&ledger, 1000);
    let state = fixture.read();
    assert!(
        state["buckets"]
            .as_object()
            .unwrap()
            .values()
            .all(|bucket| bucket["count"] == 2)
    );
    let bytes = fs::read_to_string(&fixture.path).unwrap();
    assert!(bytes.contains(r#""binding":{"scope":"login-test","capacity":5,"windowMs":60000}"#));
    let vectors: Value = serde_json::from_str(include_str!("node-vectors.json")).unwrap();
    let reservation: Vec<Reservation> =
        serde_json::from_value(vectors["reservation"].clone()).unwrap();
    ledger.complete(&reservation, 2000).unwrap();
    assert!(
        fixture.read()["buckets"]
            .as_object()
            .unwrap()
            .values()
            .all(|bucket| bucket["completedAt"] == 2000)
    );
}

#[test]
fn restart_inherits_uncompleted_and_failed_attempts_without_a_refund() {
    let fixture = Fixture::new();
    {
        let ledger = fixture.ledger();
        for _ in 0..4 {
            fixture.reserve(&ledger, 1000);
        }
        let failed_4xx = fixture.reserve(&ledger, 1000);
        ledger.complete(&failed_4xx, 3000).unwrap();
    }
    let ledger = fixture.ledger();
    let result = ledger
        .try_reserve("system", "admin", "198.18.1.2", 3000)
        .unwrap();
    assert_eq!(result.wait_ms, 60_250);
    assert!(result.reservation.is_none());
    let state = fixture.read();
    assert_eq!(state["observedAt"], 3000);
    assert!(
        state["buckets"]
            .as_object()
            .unwrap()
            .values()
            .all(|bucket| bucket["count"] == 5)
    );
    assert!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", 63_250)
            .unwrap()
            .reservation
            .is_some()
    );
}

#[test]
fn independent_controllers_serialize_two_plus_three_reservations_without_lost_counts() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let threads = [2, 3]
        .into_iter()
        .map(|count| {
            let barrier = barrier.clone();
            let ledger = fixture.ledger();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..count {
                    assert!(
                        ledger
                            .try_reserve("system", "admin", "198.18.1.2", 1000)
                            .unwrap()
                            .reservation
                            .is_some()
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    let state = fixture.read();
    assert!(
        state["buckets"]
            .as_object()
            .unwrap()
            .values()
            .all(|bucket| bucket["count"] == 5)
    );
    assert_eq!(
        fixture
            .ledger()
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .unwrap()
            .wait_ms,
        60_250
    );
}

#[test]
fn binding_changes_fail_without_modifying_existing_bytes() {
    let fixture = Fixture::new();
    fixture.attempt(&fixture.ledger(), 1000);
    let before = fs::read(&fixture.path).unwrap();
    for binding in [
        Binding::new("another-scope", 5, 60_000).unwrap(),
        Binding::new("login-test", 6, 60_000).unwrap(),
        Binding::new("login-test", 5, 61_000).unwrap(),
    ] {
        let ledger = LedgerFile::new(fixture.path.clone(), binding).unwrap();
        assert_eq!(
            ledger
                .try_reserve("system", "admin", "198.18.1.2", 1000)
                .unwrap_err(),
            BudgetError::Ledger
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
    }
    assert!(LedgerFile::new("relative.json".into(), Binding::new("a1", 1, 1).unwrap()).is_err());
    let malformed = Binding {
        scope: "a1".into(),
        capacity: 0,
        window_ms: 1,
    };
    assert!(LedgerFile::new(fixture.path, malformed).is_err());
}

#[test]
fn corrupt_future_oversized_and_reordered_binding_files_are_preserved() {
    let fixture = Fixture::new();
    let source = include_str!("node-ledger.json");
    for content in [
        "{".to_owned(),
        source.replace("\"observedAt\":1000", "\"observedAt\":1001"),
        source.replace("\"reservedAt\":1000", "\"reservedAt\":1001"),
        source.replace("\"completedAt\":1000", "\"completedAt\":1001"),
        source.replace(
            "\"scope\":\"login-test\",\"capacity\":5",
            "\"capacity\":5,\"scope\":\"login-test\"",
        ),
        source.replace("principal:", "private-marker:"),
        " ".repeat(256 * 1024 + 1),
    ] {
        fs::write(&fixture.path, content.as_bytes()).unwrap();
        let error = fixture
            .ledger()
            .try_reserve("system", "private-marker", "198.18.1.2", 1000)
            .unwrap_err();
        assert!(!error.to_string().contains("private-marker"));
        assert!(!error.to_string().contains(fixture.path.to_str().unwrap()));
        assert_eq!(fs::read(&fixture.path).unwrap(), content.as_bytes());
        assert_eq!(
            fs::read_dir(fixture.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn exact_read_limit_is_accepted_and_directory_is_not_replaced() {
    let fixture = Fixture::new();
    let mut bytes = include_bytes!("node-ledger.json").to_vec();
    bytes.resize(256 * 1024, b' ');
    fs::write(&fixture.path, bytes).unwrap();
    fixture.attempt(&fixture.ledger(), 1000);
    fs::remove_file(&fixture.path).unwrap();
    fs::create_dir(&fixture.path).unwrap();
    assert_eq!(
        fixture
            .ledger()
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .unwrap_err(),
        BudgetError::File
    );
    assert!(fixture.path.is_dir());
}

#[test]
fn invalid_old_or_partial_reservations_and_rollback_never_rewrite_the_ledger() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let old = fixture.reserve(&ledger, 1000);
    let before = fs::read(&fixture.path).unwrap();
    let mut invalid = old.clone();
    invalid[1].generation = "00000000-0000-0000-0000-000000000000".into();
    assert_eq!(
        ledger.complete(&invalid, 2000),
        Err(BudgetError::Reservation)
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    assert_eq!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", 999)
            .unwrap_err(),
        BudgetError::Ledger
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    fixture.reserve(&ledger, 61_250);
    let before = fs::read(&fixture.path).unwrap();
    assert_eq!(ledger.complete(&old, 61_250), Err(BudgetError::Reservation));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn final_and_parent_symlinks_never_modify_external_ledger_files() {
    let fixture = Fixture::new();
    let target = fixture.path.with_file_name("external.json");
    fs::write(&target, include_bytes!("node-ledger.json")).unwrap();
    std::os::unix::fs::symlink(&target, &fixture.path).unwrap();
    assert!(
        fixture
            .ledger()
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .is_err()
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        include_bytes!("node-ledger.json")
    );
    fs::remove_file(&fixture.path).unwrap();
    let actual = fixture.path.parent().unwrap().join("actual");
    fs::create_dir(&actual).unwrap();
    fs::write(
        actual.join("budget.json"),
        include_bytes!("node-ledger.json"),
    )
    .unwrap();
    let link = fixture.path.parent().unwrap().join("linked");
    std::os::unix::fs::symlink(&actual, &link).unwrap();
    let ledger = LedgerFile::new(
        link.join("budget.json"),
        Binding::new("login-test", 5, 60_000).unwrap(),
    )
    .unwrap();
    assert!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .is_err()
    );
    assert_eq!(
        fs::read(actual.join("budget.json")).unwrap(),
        include_bytes!("node-ledger.json")
    );
    assert_eq!(fs::read_dir(actual).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn directory_permission_failure_preserves_the_existing_ledger() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    fixture.attempt(&ledger, 1000);
    let before = fs::read(&fixture.path).unwrap();
    let parent = fixture.path.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
    let result = ledger.try_reserve("system", "admin", "198.18.1.2", 1000);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn read_only_destination_preserves_bytes_and_unknown_publication_stops_replay() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    fixture.attempt(&ledger, 1000);
    let before = fs::read(&fixture.path).unwrap();
    let original = fs::metadata(&fixture.path).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&fixture.path, read_only).unwrap();
    let result = ledger.try_reserve("system", "admin", "198.18.1.2", 1000);
    fs::set_permissions(&fixture.path, original).unwrap();
    assert_eq!(result.unwrap_err(), BudgetError::PublicationUnknown);
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    assert_eq!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .unwrap_err(),
        BudgetError::PublicationUnknown
    );
    assert!(
        fs::read_dir(fixture.path.parent().unwrap())
            .unwrap()
            .any(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "tmp")
            })
    );
}

use std::fs;

use super::*;

fn fixture() -> (tempfile::TempDir, LedgerFile) {
    let local = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/login-budget-controller");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let ledger = LedgerFile::new(
        directory.path().join("budget.json"),
        Binding::new("login-test", 5, 60_000).unwrap(),
    )
    .unwrap();
    (directory, ledger)
}

#[test]
fn loss_of_lock_ownership_preserves_the_lock_and_blocks_subsequent_mutations() {
    let (_directory, ledger) = fixture();
    let lock_path = paths::appended(&ledger.path, ".lock");
    let selected = model::login_keys("system", "admin", "198.18.1.2").unwrap();
    let result = ledger.update(1000, |state| {
        state.reserve(&ledger.binding, &selected, 1000)?;
        fs::write(&lock_path, b"external-owner").map_err(|_| BudgetError::Io)?;
        Ok(())
    });
    assert_eq!(result, Err(BudgetError::PublicationUnknown));
    assert!(!ledger.path.exists());
    assert_eq!(fs::read(&lock_path).unwrap(), b"external-owner");
    assert_eq!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", 1000)
            .unwrap_err(),
        BudgetError::PublicationUnknown
    );
    assert_eq!(
        ledger.complete(&[], 1000),
        Err(BudgetError::PublicationUnknown)
    );
    assert_eq!(fs::read(lock_path).unwrap(), b"external-owner");
}

#[test]
fn invalid_identity_and_unsafe_time_fail_before_creating_any_lock_or_ledger() {
    let (directory, ledger) = fixture();
    assert_eq!(
        ledger
            .try_reserve("system", " ", "198.18.1.2", 1000)
            .unwrap_err(),
        BudgetError::Identity
    );
    assert_eq!(
        ledger
            .try_reserve("system", "admin", "198.18.1.2", u64::MAX)
            .unwrap_err(),
        BudgetError::Clock
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn all_error_messages_are_fixed_and_do_not_include_io_or_configuration_values() {
    for error in [
        BudgetError::Scope,
        BudgetError::Capacity,
        BudgetError::Window,
        BudgetError::Identity,
        BudgetError::Clock,
        BudgetError::Ledger,
        BudgetError::Bucket,
        BudgetError::BucketsFull,
        BudgetError::Reservation,
        BudgetError::Path,
        BudgetError::File,
        BudgetError::Size,
        BudgetError::Io,
        BudgetError::LockBusy,
        BudgetError::LockOwner,
        BudgetError::PublicationUnknown,
    ] {
        assert_eq!(error.to_string(), error.message());
        assert!(!error.to_string().contains("private-marker"));
    }
}

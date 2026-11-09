use super::*;

fn control_dir() -> Result<(tempfile::TempDir, PathBuf)> {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let temporary = tempfile::tempdir_in(local)?;
    let control = super::super::portable_path(&temporary.path().canonicalize()?);
    Ok((temporary, control))
}

fn owner_record(control: &Path, operation: &str, process: &Value) -> Value {
    json!({
        "format_version":1, "kind":super::super::OWNER_KIND, "identity":process,
        "runtime_directory":control, "operation":operation, "token":"a".repeat(32),
    })
}

fn put_owner(control: &Path, owner: &Value) -> Result<PathBuf> {
    let lock = control.join(REGISTRATION_LOCK);
    fs::create_dir(&lock)?;
    let path = lock.join("owner.json");
    strict::write_new(&path, owner)?;
    Ok(path)
}

#[test]
fn observed_pid_reuse_requires_a_distinct_trustworthy_creation_time() -> Result<()> {
    let executable = std::env::current_exe()?;
    let owner = json!({"pid":200,"started":"512","executable":executable});
    let mut actual = owner.clone();
    assert_eq!(stopped_identity(&owner, None)?, None);
    assert!(stopped_identity(&owner, Some(actual.clone())).is_err());
    actual["executable"] = json!(executable.with_file_name("other-controller.exe"));
    assert!(stopped_identity(&owner, Some(actual.clone())).is_err());
    actual["started"] = json!("0512");
    assert!(stopped_identity(&owner, Some(actual.clone())).is_err());
    actual["started"] = json!("513");
    assert_eq!(
        stopped_identity(&owner, Some(actual.clone()))?,
        Some(actual)
    );
    Ok(())
}

#[test]
fn recover_archives_stale_owner_and_claims_replacement_under_the_same_guard() -> Result<()> {
    let (_temporary, control) = control_dir()?;
    let start = "runtime-start:registered:target";
    let recover = "runtime-recover:registered:target";
    let operations = [start.to_owned(), recover.to_owned()];
    let process = json!({"pid":u32::MAX,"started":"1","executable":std::env::current_exe()?});
    assert!(identity::process(u32::MAX)?.is_none());
    let original = owner_record(&control, start, &process);
    let path = put_owner(&control, &original)?;
    let lock_id = directory_id(path.parent().unwrap())?;
    let recovery = Owner::recover_with(
        &control,
        recover,
        &path,
        &operations,
        |value| {
            assert_eq!(value, &original);
            assert_eq!(strict::read(&path)?, original);
            assert!(ProcessGuard::acquire_named(&control, REGISTRATION_GUARD).is_err());
            Ok(7)
        },
        |owner, recovery, prechecked| {
            owner.check()?;
            assert_eq!(prechecked, 7);
            assert!(ProcessGuard::acquire_named(&control, REGISTRATION_GUARD).is_err());
            let current = strict::read(&path)?;
            assert_eq!(current["identity"], identity::current()?);
            assert_eq!(current["operation"], recover);
            assert_ne!(current["token"], original["token"]);
            assert_eq!(directory_id(path.parent().unwrap())?, lock_id);
            let token = strict::text(&current["token"], "测试新 token")?;
            assert_eq!(recovery["receipt"], json!(archive_path(&control, token)));
            assert_eq!(Owner::prior_recovered_owner(&control, &current)?, original);
            Ok(recovery)
        },
    )?;
    let archive = PathBuf::from(strict::text(&recovery["receipt"], "测试归档路径")?);
    assert_eq!(archive.parent(), Some(control.as_path()));
    assert!(
        archive
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("restore-registration-reconcile-")
    );
    assert_eq!(
        strict::read(&archive)?,
        json!({
            "format_version":1, "kind":"restore-runtime-registration-reconciliation",
            "owner":original, "observed_identity":null, "remote_writes_reconciled":false,
        })
    );
    assert!(!path.exists());
    assert!(!control.join(REGISTRATION_LOCK).exists());
    assert!(control.join(REGISTRATION_GUARD).is_file());
    Ok(())
}

#[test]
fn recover_leaves_live_or_foreign_owner_and_never_enters_recover_body() -> Result<()> {
    let (_temporary, control) = control_dir()?;
    let operation = "runtime-recover:registered:target";
    let operations = [operation.to_owned()];
    let original = owner_record(&control, operation, &identity::current()?);
    let path = put_owner(&control, &original)?;
    let wrong = control.join("other-owner.json");
    strict::write_new(&wrong, &original)?;
    for selected in [&wrong, &path] {
        assert!(
            Owner::recover_with::<(), ()>(
                &control,
                operation,
                selected,
                &operations,
                |_| panic!("必须先证明原 owner 已退出"),
                |_, _, ()| panic!("原 owner 未关闭时不能执行恢复"),
            )
            .is_err()
        );
    }
    assert_eq!(strict::read(&path)?, original);
    assert_eq!(strict::read(&wrong)?, original);
    assert!(fs::read_dir(&control)?.all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(RECOVERY_PREFIX)
    }));
    Ok(())
}

#[test]
fn failed_recover_retains_new_owner_and_its_exact_predecessor() -> Result<()> {
    let (_temporary, control) = control_dir()?;
    let start = "runtime-start:registered:target";
    let operation = "runtime-recover:registered:target";
    let operations = [start.to_owned(), operation.to_owned()];
    let dead = json!({"pid":u32::MAX,"started":"1","executable":std::env::current_exe()?});
    let original = owner_record(&control, start, &dead);
    let path = put_owner(&control, &original)?;
    let error = Owner::recover_with::<(), ()>(
        &control,
        operation,
        &path,
        &operations,
        |_| Ok(()),
        |_, _, ()| Err("首次状态发布失败".into()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("首次状态发布失败"));
    assert!(error.to_string().contains("新恢复 owner 已保留"));
    let _guard = ProcessGuard::acquire_named(&control, REGISTRATION_GUARD)?;
    let current = strict::read(&path)?;
    assert_eq!(current["operation"], operation);
    assert_eq!(Owner::prior_recovered_owner(&control, &current)?, original);

    let mut unlinked = current;
    unlinked["token"] = json!("f".repeat(32));
    assert!(Owner::prior_recovered_owner(&control, &unlinked).is_err());
    Ok(())
}

#[test]
fn predecessor_archive_rejects_ambiguous_identity_and_remote_writes_claim() -> Result<()> {
    let (_temporary, control) = control_dir()?;
    let current = new_receipt(
        &control,
        "runtime-recover:registered:target",
        &"b".repeat(32),
    )?;
    let executable = std::env::current_exe()?;
    let identity = json!({"pid":200,"started":"512","executable":executable});
    let original = owner_record(&control, "runtime-start:registered:target", &identity);
    let path = archive_path(&control, &"b".repeat(32));
    let mut archive = json!({
        "format_version":1, "kind":RECOVERY_KIND, "owner":original,
        "observed_identity":{"pid":200,"started":"512","executable":executable.with_file_name("different.exe")},
        "remote_writes_reconciled":false,
    });
    strict::write_new(&path, &archive)?;
    assert!(Owner::prior_recovered_owner(&control, &current).is_err());

    archive["observed_identity"]["started"] = json!("513");
    archive["remote_writes_reconciled"] = json!(true);
    strict::write_replace(&path, &archive)?;
    assert!(Owner::prior_recovered_owner(&control, &current).is_err());

    archive["remote_writes_reconciled"] = json!(false);
    strict::write_replace(&path, &archive)?;
    assert_eq!(Owner::prior_recovered_owner(&control, &current)?, original);
    Ok(())
}

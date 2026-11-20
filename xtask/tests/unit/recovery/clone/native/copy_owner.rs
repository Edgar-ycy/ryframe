use super::*;

#[path = "copy_owner_recovery_guards.rs"]
mod recovery_guards;

const SESSION_ID: &str = "session-0123456789abcdef0123456789abcdef";

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    copy: PathBuf,
    target: PathBuf,
    source_binding: Value,
    initialized_binding: Value,
    owner_value: Value,
    owner_binding: Value,
}

impl Fixture {
    fn new(current_session_shape: bool) -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let backend = portable_path(&fs::canonicalize(directory.path()).unwrap());
        let local = backend.join(".local-tests");
        let copy = local.join("copy");
        let target = local.join("target");
        fs::create_dir_all(copy.join("ledger")).unwrap();
        fs::create_dir(&target).unwrap();
        let source_binding = write(&local.join("export.json"), &json!({"source":true}));
        let initialized_binding = write(&target.join("initialized.json"), &json!({"target":true}));
        let mut saved = json!({
            "source_export":source_binding, "initialized":initialized_binding,
            "plan_sha256":"a".repeat(64), "generation_sha256":"b".repeat(64)
        });
        if current_session_shape {
            saved["source_observation"] = json!({});
            saved["clone_verified"] = json!(false);
            saved["restore_qualified"] = json!(false);
        }
        let session = write(&copy.join("session.json"), &saved);
        let controller = json!({
            "pid":8309, "started":"133234570059900000",
            "executable":backend.join("old-controller.exe")
        });
        let owner_value = json!({
            "format_version":1, "kind":"devex-copy-owner", "backend_root":backend,
            "copy_directory":copy, "session":session, "session_id":SESSION_ID,
            "source_export":source_binding, "initialized":initialized_binding,
            "plan_sha256":saved["plan_sha256"], "generation_sha256":saved["generation_sha256"],
            "controller":controller
        });
        let owner_binding = write(
            &copy.join("copy-owner-0123456789abcdef0123456789abcdef.json"),
            &owner_value,
        );
        Self {
            _directory: directory,
            backend,
            copy,
            target,
            source_binding,
            initialized_binding,
            owner_value,
            owner_binding,
        }
    }

    fn owner(&self) -> Result<Owner> {
        Owner::read(
            &self.backend,
            &self.copy,
            &self.owner_binding,
            &self.source_binding,
            &self.initialized_binding,
        )
    }

    fn register(&self) -> Result<Owner> {
        Owner::register(
            &self.backend,
            &self.copy,
            &self.source_binding,
            &self.initialized_binding,
        )
    }

    fn add_lock(&self, role: Role, owner: &Owner) -> Value {
        let path = owner.lock_path(role);
        fs::create_dir(path).unwrap();
        let (device, inode) =
            common::device_inode_for(path, &fs::symlink_metadata(path).unwrap()).unwrap();
        let record = json!({
            "format_version":1, "kind":"devex-owned-copy-lock", "role":role.name(),
            "owner_receipt":self.owner_binding, "controller":self.owner_value["controller"],
            "path":common::path_text(path, "复制测试锁").unwrap(), "device":device, "inode":inode
        });
        write(&path.join("owner.json"), &record);
        record
    }

    fn command(&self, suffix: &str) -> PathBuf {
        self.copy
            .join(format!("command-{SESSION_ID}-{suffix}.json"))
    }

    fn bind_controller(&mut self, controller: Value) {
        self.owner_value["controller"] = controller;
        let path = Path::new(self.owner_binding["path"].as_str().unwrap());
        json_io::write_replace(path, &self.owner_value).unwrap();
        self.owner_binding = common::binding(path).unwrap();
    }
}

fn write(path: &Path, value: &Value) -> Value {
    json_io::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

#[test]
fn registration_publishes_current_owner_and_preserves_bound_session_and_old_receipt() {
    let fixture = Fixture::new(true);
    let session = fixture.copy.join("session.json");
    let original_session = fs::read(&session).unwrap();
    let original_receipt = Path::new(fixture.owner_binding["path"].as_str().unwrap());
    let original_owner = fs::read(original_receipt).unwrap();
    let owner = fixture.register().unwrap();
    let registered = common::read_bound(&fixture.backend, owner.binding(), "测试登记").unwrap();
    let suffix = owner.session_id().strip_prefix("session-").unwrap();
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let owner_path = fixture.copy.join(format!("copy-owner-{suffix}.json"));
    assert_eq!(*owner.binding(), common::binding(&owner_path).unwrap());
    assert_eq!(registered["kind"], "devex-copy-owner");
    assert_eq!(registered["session"], common::binding(&session).unwrap());
    assert_eq!(registered["controller"], identity::current().unwrap());
    let (backend, plan, generation) = owner.ledger_binding();
    assert_eq!(backend, fixture.backend);
    assert_eq!(plan, "a".repeat(64));
    assert_eq!(generation, "b".repeat(64));
    let saved = Owner::read(
        &fixture.backend,
        &fixture.copy,
        owner.binding(),
        &fixture.source_binding,
        &fixture.initialized_binding,
    )
    .unwrap();
    assert_eq!(saved.ledger_binding(), owner.ledger_binding());
    assert_eq!(saved.session_id(), owner.session_id());

    let alternate = write(
        &fixture.copy.join("other-export.json"),
        &json!({"source":true}),
    );
    let before = fs::read_dir(&fixture.copy).unwrap().count();
    assert!(
        Owner::register(
            &fixture.backend,
            &fixture.copy,
            &alternate,
            &fixture.initialized_binding
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(&fixture.copy).unwrap().count(), before);
    assert_eq!(fs::read(&session).unwrap(), original_session);
    assert_eq!(fs::read(original_receipt).unwrap(), original_owner);
}

#[test]
fn live_claim_and_release_require_registered_controller_and_exact_guard() {
    let fixture = Fixture::new(false);
    let historical = fixture.owner().unwrap();
    let owner = fixture.register().unwrap();
    let target = owner.lock_path(Role::Target);
    let ledger = owner.lock_path(Role::Ledger);
    let target_guard =
        ProcessGuard::acquire_named(&fixture.target, Role::Target.guard_name()).unwrap();
    let wrong_parent =
        ProcessGuard::acquire_named(&fixture.copy, Role::Target.guard_name()).unwrap();
    let wrong_name = ProcessGuard::acquire_named(&fixture.target, "unrelated.guard").unwrap();
    let ledger_guard =
        ProcessGuard::acquire_named(ledger.parent().unwrap(), Role::Ledger.guard_name()).unwrap();

    for guard in [&wrong_parent, &wrong_name, &ledger_guard] {
        assert!(owner.claim_lock(Role::Target, guard).is_err());
    }
    assert!(historical.claim_lock(Role::Target, &target_guard).is_err());
    assert!(!target.exists());
    fs::create_dir(target).unwrap();
    assert!(owner.claim_lock(Role::Target, &target_guard).is_err());
    assert!(fs::read_dir(target).unwrap().next().is_none());
    fs::remove_dir(target).unwrap();

    let record = owner.claim_lock(Role::Target, &target_guard).unwrap();
    let ledger_record = owner.claim_lock(Role::Ledger, &ledger_guard).unwrap();
    let receipt = target.join("owner.json");
    let original = fs::read(&receipt).unwrap();
    assert!(owner.claim_lock(Role::Target, &target_guard).is_err());
    assert!(
        owner
            .check_live_lock(Role::Target, &wrong_parent, &record)
            .is_err()
    );
    assert!(
        owner
            .release_lock(Role::Target, &wrong_name, &record)
            .is_err()
    );
    let mut mismatched = record.clone();
    mismatched["owner_receipt"] = fixture.owner_binding;
    assert!(
        owner
            .release_lock(Role::Target, &target_guard, &mismatched)
            .is_err()
    );
    assert_eq!(fs::read(&receipt).unwrap(), original);
    owner
        .check_live_lock(Role::Target, &target_guard, &record)
        .unwrap();
    owner
        .release_lock(Role::Target, &target_guard, &record)
        .unwrap();
    assert!(!target.exists());
    owner
        .check_live_lock(Role::Ledger, &ledger_guard, &ledger_record)
        .unwrap();
    owner
        .release_lock(Role::Ledger, &ledger_guard, &ledger_record)
        .unwrap();
    assert!(!ledger.exists());
}

#[test]
fn release_rollback_restores_exact_bytes_and_refuses_to_replace_members() {
    let fixture = Fixture::new(false);
    let owner = fixture.register().unwrap();
    let role = Role::Target;
    let path = owner.lock_path(role);
    let guard = ProcessGuard::acquire_named(&fixture.target, role.guard_name()).unwrap();
    let record = owner.claim_lock(role, &guard).unwrap();
    let receipt = path.join("owner.json");
    let raw = format!("  {}\n\n", serde_json::to_string(&record).unwrap()).into_bytes();
    fs::remove_file(&receipt).unwrap();
    restore_owner(path, &receipt, &record, &raw).unwrap();
    assert_eq!(fs::read(&receipt).unwrap(), raw);
    assert!(restore_owner(path, &receipt, &record, b"other bytes").is_err());
    assert_eq!(fs::read(&receipt).unwrap(), raw);
    owner.release_lock(role, &guard, &record).unwrap();
}

#[test]
fn recovery_holds_both_guards_and_removes_only_owned_locks_after_direct_command_exit() {
    let mut fixture = Fixture::new(true);
    let mut historical = identity::current().unwrap();
    historical["started"] = json!("1");
    fixture.bind_controller(historical);
    let owner = fixture.owner().unwrap();
    let target = fixture.add_lock(Role::Target, &owner);
    let ledger = fixture.add_lock(Role::Ledger, &owner);
    let events = fixture.copy.join("ledger/events.ndjson");
    fs::write(&events, b"original journal").unwrap();
    let command = fixture.command("running");
    write(&command, &json!({"returncode":null}));
    let checkpoint = || Ok(());
    let recover = || {
        super::super::copy_recover::reclaim(
            &fixture.copy,
            &owner,
            &fixture.owner_binding,
            &checkpoint,
        )
    };
    assert!(recover().is_err());
    assert_eq!(
        owner.verify_lock(Role::Target).unwrap(),
        Some(target.clone())
    );
    assert_eq!(
        owner.verify_lock(Role::Ledger).unwrap(),
        Some(ledger.clone())
    );
    assert!(!fs::read_dir(&fixture.copy).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("copy-lock-recovery-")
    }));
    json_io::write_replace(&command, &json!({"returncode":-1})).unwrap();

    let result = recover().unwrap();
    assert_eq!(result["status"], "owned_locks_released");
    assert_eq!(result["locks"], json!([target, ledger]));
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["remote_writes_reconciled"], false);
    assert_eq!(result["requires_reconcile"], true);
    assert_eq!(result["observed_controller"], identity::current().unwrap());
    let mut published = result;
    let receipt = published
        .as_object_mut()
        .unwrap()
        .remove("receipt")
        .unwrap();
    assert_eq!(
        common::read_bound(&fixture.backend, &receipt, "测试回收收据").unwrap(),
        published
    );
    assert!(owner.verify_lock(Role::Target).unwrap().is_none());
    assert!(owner.verify_lock(Role::Ledger).unwrap().is_none());
    assert_eq!(fs::read(events).unwrap(), b"original journal");
}

#[test]
fn recovery_rejects_a_controller_with_the_current_creation_identity() {
    let mut fixture = Fixture::new(false);
    fixture.bind_controller(identity::current().unwrap());
    let owner = fixture.owner().unwrap();
    let ledger = fixture.add_lock(Role::Ledger, &owner);
    let result =
        super::super::copy_recover::reclaim(&fixture.copy, &owner, &fixture.owner_binding, &|| {
            Ok(())
        });
    assert!(result.is_err());
    assert_eq!(owner.verify_lock(Role::Ledger).unwrap(), Some(ledger));
}

#[test]
fn sealed_owner_binds_original_session_and_both_lock_records_without_writes() {
    for current_shape in [false, true] {
        let fixture = Fixture::new(current_shape);
        let owner = fixture.owner().unwrap();
        assert_eq!(owner.session_id(), SESSION_ID);
        assert_eq!(owner.controller(), &fixture.owner_value["controller"]);
        assert_eq!(
            owner.lock_path(Role::Target),
            fixture.target.join("initialize.lock")
        );
        assert_eq!(
            owner.lock_path(Role::Ledger),
            fixture.copy.join("ledger/clone.lock")
        );
        assert_eq!(Role::Target.guard_name(), "copy-target.guard");
        assert_eq!(Role::Ledger.guard_name(), "copy-ledger.guard");
        assert!(owner.verify_lock(Role::Target).unwrap().is_none());
        assert!(owner.verify_lock(Role::Ledger).unwrap().is_none());

        for role in [Role::Target, Role::Ledger] {
            let record = fixture.add_lock(role, &owner);
            let path = owner.lock_path(role).join("owner.json");
            let before = fs::read(&path).unwrap();
            assert_eq!(owner.verify_lock(role).unwrap(), Some(record));
            assert_eq!(fs::read(path).unwrap(), before);
        }
        let command = fixture.command("complete");
        write(&command, &json!({"returncode":-9}));
        fs::write(
            fixture.copy.join("command-session-old-unrelated.json"),
            b"{",
        )
        .unwrap();
        let before = fs::read(&command).unwrap();
        let commands = owner.verify_commands().unwrap();
        commands.check().unwrap();
        assert_eq!(fs::read(command).unwrap(), before);
        owner.check().unwrap();
    }
}

#[test]
fn bare_other_owner_and_changed_directory_identity_are_rejected() {
    let fixture = Fixture::new(true);
    let owner = fixture.owner().unwrap();
    let path = owner.lock_path(Role::Ledger);
    let before = fixture.add_lock(Role::Ledger, &owner);
    let owner_file = path.join("owner.json");
    fs::remove_file(&owner_file).unwrap();
    assert!(owner.verify_lock(Role::Ledger).is_err());
    for field in [
        "controller",
        "owner_receipt",
        "device",
        "inode",
        "role",
        "path",
    ] {
        let mut record = before.clone();
        record[field] = Value::Null;
        fs::write(&owner_file, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(owner.verify_lock(Role::Ledger).is_err(), "{field}");
    }
    fs::write(&owner_file, serde_json::to_vec(&before).unwrap()).unwrap();
    write(&path.join("foreign.json"), &json!({}));
    assert!(owner.verify_lock(Role::Ledger).is_err());
    assert!(owner_file.is_file());

    let previous = path.with_file_name("previous-lock");
    fs::rename(path, &previous).unwrap();
    fs::create_dir(path).unwrap();
    write(&owner_file, &before);
    assert!(owner.verify_lock(Role::Ledger).is_err());
    assert_eq!(json_io::read(&owner_file).unwrap(), before);
}

#[test]
fn owner_requires_the_caller_fixed_source_initialized_and_session() {
    let fixture = Fixture::new(true);
    let wrong = write(&fixture.copy.join("other.json"), &json!({"different":true}));
    for (source, initialized) in [
        (&wrong, &fixture.initialized_binding),
        (&fixture.source_binding, &wrong),
    ] {
        assert!(
            Owner::read(
                &fixture.backend,
                &fixture.copy,
                &fixture.owner_binding,
                source,
                initialized
            )
            .is_err()
        );
    }
    assert!(
        Owner::read(
            &fixture.backend,
            &fixture.target,
            &fixture.owner_binding,
            &fixture.source_binding,
            &fixture.initialized_binding
        )
        .is_err()
    );
    fs::write(fixture.copy.join("session.json"), b"{}").unwrap();
    assert!(fixture.owner().is_err());
}

#[test]
fn commands_reject_invalid_exit_codes_malicious_paths_and_changed_membership() {
    let fixture = Fixture::new(true);
    let owner = fixture.owner().unwrap();
    let path = fixture.command("pending");
    for bytes in [
        b"".as_slice(),
        b"{",
        br#"{"returncode":null}"#,
        br#"{"returncode":false}"#,
        br#"{"returncode":1.5}"#,
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(owner.verify_commands().is_err());
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(json_io::MAX_JSON_BYTES + 1)
        .unwrap();
    assert!(owner.verify_commands().is_err());

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(owner.verify_commands().is_err());
    fs::remove_dir(&path).unwrap();

    let unrelated = fixture.copy.join("unrelated.json");
    write(&unrelated, &json!({"returncode":0}));
    fs::hard_link(unrelated, &path).unwrap();
    assert!(owner.verify_commands().is_err());
    fs::remove_file(&path).unwrap();

    write(&path, &json!({"returncode":0}));
    let proof = owner.verify_commands().unwrap();
    write(&fixture.command("late"), &json!({"returncode":0}));
    assert!(proof.check().is_err());

    let mut traversal = fixture.owner_binding.clone();
    traversal["path"] = json!(
        fixture
            .copy
            .join("../copy/copy-owner-0123456789abcdef0123456789abcdef.json")
    );
    assert!(
        Owner::read(
            &fixture.backend,
            &fixture.copy,
            &traversal,
            &fixture.source_binding,
            &fixture.initialized_binding
        )
        .is_err()
    );
}

#[test]
fn owner_keeps_frozen_source_and_session_inputs_protected() {
    let fixture = Fixture::new(true);
    let owner = fixture.owner().unwrap();
    let source = fixture.backend.join(".local-tests/export.json");
    let before = fs::read(&source).unwrap();
    match fs::write(&source, b"{}") {
        Ok(()) => assert!(owner.check().is_err()),
        Err(error) => {
            assert!(
                cfg!(windows)
                    && (error.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(error.raw_os_error(), Some(32 | 33))),
                "{error}"
            );
            assert_eq!(fs::read(&source).unwrap(), before);
            owner.check().unwrap();
        }
    }
}

#[test]
fn live_creation_identity_cannot_be_treated_as_stopped_by_different_image_spelling() {
    let fixture = Fixture::new(true);
    let mut value = fixture.owner_value.clone();
    let mut controller = identity::current().unwrap();
    controller["executable"] = json!(fixture.backend.join("different-recorded-image.exe"));
    value["controller"] = controller;
    let binding = write(&fixture.copy.join("copy-owner-live.json"), &value);
    let owner = Owner::read(
        &fixture.backend,
        &fixture.copy,
        &binding,
        &fixture.source_binding,
        &fixture.initialized_binding,
    )
    .unwrap();
    assert!(
        owner
            .observe_stopped()
            .unwrap_err()
            .to_string()
            .contains("仍在运行")
    );
}

use super::*;

fn plan() -> Plan {
    Plan {
        schema: "ryframe_it_scope_case_123_1".into(),
        scope: "fixture-scope".into(),
        test: "fixture-case".into(),
        identity: Identity {
            server_uuid: "10000000-0000-4000-8000-000000000001".into(),
            connection_id: 42,
        },
    }
}

fn observation() -> Observation {
    Observation {
        identity: plan().identity,
        lock_owner: Some(42),
        plan_identity_matches: true,
        plan_content_matches: true,
        journal_matches: true,
        business_closed: true,
        schema_exists: true,
        process_visibility: true,
        active_connections: 0,
    }
}

fn ready() -> Ownership {
    let mut owner = Ownership::new(plan()).unwrap();
    owner.create_intent().unwrap();
    owner.created().unwrap();
    owner.test_passed().unwrap();
    owner
}

#[test]
fn confirmed_create_successful_test_and_complete_evidence_allow_one_drop() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    assert_eq!(owner.phase, Phase::DropIntent);
    owner.validate_cleanup(&observation()).unwrap();
    owner.dropped(true).unwrap();
    assert_eq!(owner.phase, Phase::Dropped);
    assert!(owner.drop_intent(&observation()).is_err());
}

#[test]
fn creation_intent_without_create_confirmation_never_authorizes_cleanup() {
    let mut owner = Ownership::new(plan()).unwrap();
    owner.create_intent().unwrap();
    assert!(owner.drop_intent(&observation()).is_err());
    assert_eq!(owner.phase, Phase::Retained);
    assert!(owner.created().is_err());
    assert!(owner.test_passed().is_err());
}

#[test]
fn create_response_unknown_or_created_journal_failure_cannot_be_replayed() {
    for confirmed in [false, true] {
        let mut owner = Ownership::new(plan()).unwrap();
        owner.create_intent().unwrap();
        if confirmed {
            owner.created().unwrap();
        }
        owner.retain();
        assert!(owner.create_intent().is_err());
        assert!(owner.created().is_err());
        assert!(owner.test_passed().is_err());
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn failed_test_or_panic_retains_created_schema_without_drop() {
    for _outcome in ["error", "panic"] {
        let mut owner = Ownership::new(plan()).unwrap();
        owner.create_intent().unwrap();
        owner.created().unwrap();
        owner.retain();
        assert!(owner.test_passed().is_err());
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn created_schema_without_successful_test_is_not_cleanup_authority() {
    let mut owner = Ownership::new(plan()).unwrap();
    owner.create_intent().unwrap();
    owner.created().unwrap();
    assert!(owner.drop_intent(&observation()).is_err());
    assert!(owner.test_passed().is_err());
}

#[test]
fn server_uuid_or_connection_change_cannot_be_repaired_by_a_later_retry() {
    for server_changed in [false, true] {
        let mut owner = ready();
        let mut changed = observation();
        if server_changed {
            changed.identity.server_uuid.push('x');
        } else {
            changed.identity.connection_id += 1;
        }
        assert!(owner.drop_intent(&changed).is_err());
        assert_eq!(owner.phase, Phase::Retained);
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn unowned_missing_or_ambiguous_session_lock_never_allows_drop() {
    for holder in [None, Some(0), Some(41), Some(43)] {
        let mut owner = ready();
        let mut changed = observation();
        changed.lock_owner = holder;
        assert!(owner.drop_intent(&changed).is_err());
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn active_schema_connections_or_partial_process_visibility_block_drop() {
    for (active, visible) in [(1, true), (u64::MAX, true), (0, false)] {
        let mut owner = ready();
        let mut changed = observation();
        changed.active_connections = active;
        changed.process_visibility = visible;
        assert!(owner.drop_intent(&changed).is_err());
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn plan_identity_plan_content_or_journal_change_latches_retention() {
    for case in 0..3 {
        let mut owner = ready();
        let mut changed = observation();
        match case {
            0 => changed.plan_identity_matches = false,
            1 => changed.plan_content_matches = false,
            _ => changed.journal_matches = false,
        }
        assert!(owner.drop_intent(&changed).is_err());
        assert_eq!(owner.phase, Phase::Retained);
        assert!(owner.drop_intent(&observation()).is_err());
    }
}

#[test]
fn open_business_pool_or_missing_schema_forbids_drop() {
    for closed in [false, true] {
        let mut owner = ready();
        let mut changed = observation();
        changed.business_closed = closed;
        changed.schema_exists = !closed;
        assert!(owner.drop_intent(&changed).is_err());
    }
}

#[test]
fn persisted_drop_intent_and_unknown_response_never_authorize_another_drop() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    owner.retain();
    assert!(owner.drop_intent(&observation()).is_err());
    assert!(owner.dropped(true).is_err());
    assert!(owner.create_intent().is_err());
}

#[test]
fn duplicate_drop_intent_is_rejected_even_before_a_response_arrives() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    assert!(owner.drop_intent(&observation()).is_err());
    assert_eq!(owner.phase, Phase::Retained);
}

#[test]
fn test_passed_journal_failure_blocks_the_drop_entry() {
    let mut owner = ready();
    owner.retain();
    assert!(owner.drop_intent(&observation()).is_err());
    assert_eq!(owner.phase, Phase::Retained);
}

#[test]
fn drop_intent_journal_failure_does_not_authorize_execution_or_replay() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    owner.retain();
    assert!(owner.validate_cleanup(&observation()).is_err());
    assert!(owner.drop_intent(&observation()).is_err());
}

#[test]
fn drop_success_without_confirmed_absence_does_not_report_completion_or_retry() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    assert!(owner.dropped(false).is_err());
    assert_eq!(owner.phase, Phase::Retained);
    assert!(owner.drop_intent(&observation()).is_err());
}

#[test]
fn post_intent_revalidation_still_requires_all_ownership_evidence() {
    let mut owner = ready();
    owner.drop_intent(&observation()).unwrap();
    let mut changed = observation();
    changed.plan_content_matches = false;
    assert!(owner.validate_cleanup(&changed).is_err());
    owner.retain();
    assert!(owner.drop_intent(&observation()).is_err());
}

#[test]
fn database_prefix_cannot_replace_server_session_or_durable_evidence() {
    let mut owner = ready();
    assert!(owner.plan.schema.starts_with("ryframe_it_"));
    let mut changed = observation();
    changed.identity.connection_id = 99;
    changed.lock_owner = Some(99);
    assert!(owner.drop_intent(&changed).is_err());
}

#[test]
fn schema_and_lock_key_are_bounded_ascii_identifiers_before_any_ddl() {
    for schema in ["a", "schema_123", &"a".repeat(64)] {
        assert!(validate_schema(schema).is_ok());
    }
    for schema in ["", "含中文", "x`", "x-y", "x\n", &"a".repeat(65)] {
        assert!(validate_schema(schema).is_err());
    }
}

#[test]
fn plan_contains_exact_nonsecret_resource_and_session_binding() {
    let plan = plan();
    let value: serde_json::Value = serde_json::from_slice(&plan.bytes().unwrap()).unwrap();
    assert_eq!(value["schema"], plan.schema);
    assert_eq!(value["scope"], plan.scope);
    assert_eq!(value["test"], plan.test);
    assert_eq!(value["server_uuid"], plan.identity.server_uuid);
    assert_eq!(value["connection_id"], plan.identity.connection_id);
    assert_eq!(value["lock_owner"], plan.identity.connection_id);
    assert_eq!(value["lock_key"], value["schema"]);
    for forbidden in ["host", "username", "password", "dsn", "connection_string"] {
        assert!(value.get(forbidden).is_none());
    }
}

#[test]
fn process_visibility_requires_explicit_global_privileges_not_names_or_role_grants() {
    for grant in [
        "GRANT PROCESS ON *.* TO 'fixture'@'localhost'",
        "GRANT SELECT, PROCESS, SHOW DATABASES ON *.* TO 'fixture'@'localhost'",
        "GRANT ALL PRIVILEGES ON *.* TO 'fixture'@'localhost' WITH GRANT OPTION",
    ] {
        assert!(permits_processlist(grant));
    }
    for grant in [
        "GRANT USAGE ON *.* TO 'PROCESS'@'localhost'",
        "GRANT ALL PRIVILEGES ON `fixture`.* TO 'fixture'@'localhost'",
        "GRANT SELECT ON *.* TO 'PROCESS'@'localhost'",
        "GRANT `PROCESS`@`localhost` TO 'fixture'@'localhost'",
        "REVOKE PROCESS ON *.* FROM 'fixture'@'localhost'",
    ] {
        assert!(!permits_processlist(grant));
    }
}

fn io_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local-tests/mysql-fixture-ownership-offline");
    fs::create_dir_all(&parent).unwrap();
    let root = parent.join(format!(
        "{label}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    // 只创建本用例目录；失败证据与既有普通目录均保留。
    root.canonicalize().unwrap()
}

fn held_bytes(file: &mut OwnedFile) -> Vec<u8> {
    file.file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    file.file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn assert_unchanged(evidence: &mut Evidence, plan_bytes: &[u8], journal_bytes: &[u8]) {
    evidence.verify().unwrap();
    assert_eq!(held_bytes(&mut evidence.plan), plan_bytes);
    assert_eq!(held_bytes(&mut evidence.journal), journal_bytes);
}

fn assert_contended(file: &OwnedFile) {
    let other = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&file.path)
        .unwrap();
    assert!(matches!(
        other.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
}

#[cfg(windows)]
fn assert_windows_lock_denial(error: &std::io::Error) {
    assert!(
        matches!(error.raw_os_error(), Some(5 | 32 | 33)),
        "外部操作必须明确因访问或文件锁被拒绝：{error}"
    );
}

#[cfg(not(windows))]
fn assert_windows_lock_denial(error: &std::io::Error) {
    panic!("非 Windows 文件操作异常：{error}");
}

#[test]
fn evidence_file_io_records_verifies_and_remains_complete_after_handles_close() {
    let root = io_root("record");
    let metadata = plan();
    let mut evidence = Evidence::create(&root, &metadata).unwrap();
    for stage in ["offline-one", "offline-two"] {
        evidence.record(stage).unwrap();
        evidence.verify().unwrap();
    }
    evidence
        .observed("offline-observed", &observation())
        .unwrap();
    evidence.verify().unwrap();
    let plan_path = evidence.plan.path.clone();
    let journal_path = evidence.journal.path.clone();
    let plan_bytes = evidence.plan.expected.clone();
    let journal_bytes = evidence.journal.expected.clone();
    assert_eq!(plan_bytes, metadata.bytes().unwrap());
    assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
    drop(evidence);

    assert_eq!(fs::read(&plan_path).unwrap(), plan_bytes);
    assert_eq!(fs::read(&journal_path).unwrap(), journal_bytes);
    let decoded_plan: serde_json::Value = serde_json::from_slice(&plan_bytes).unwrap();
    let records: Vec<serde_json::Value> = journal_bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(records.len(), 4);
    for (record, stage) in
        records
            .iter()
            .zip(["planned", "offline-one", "offline-two", "offline-observed"])
    {
        assert_eq!(record["stage"], stage);
        for field in [
            "schema",
            "scope",
            "test",
            "server_uuid",
            "connection_id",
            "lock_key",
            "lock_owner",
        ] {
            assert_eq!(record[field], decoded_plan[field]);
        }
    }
    for value in std::iter::once(&decoded_plan).chain(&records) {
        for forbidden in ["host", "username", "password", "dsn", "connection_string"] {
            assert!(value.get(forbidden).is_none());
        }
    }
    assert_eq!(records[3]["active_connections"], 0);
    assert_eq!(records[3]["business_closed"], true);
    for path in [plan_path, journal_path] {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.try_lock().unwrap();
    }
}

#[test]
fn evidence_duplicate_schema_create_new_preserves_locked_original_files() {
    let root = io_root("duplicate");
    let mut evidence = Evidence::create(&root, &plan()).unwrap();
    evidence.record("offline-before-duplicate").unwrap();
    let plan_bytes = evidence.plan.expected.clone();
    let journal_bytes = evidence.journal.expected.clone();
    assert!(Evidence::create(&root, &plan()).is_err());
    assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
}

#[test]
fn evidence_preexisting_plan_or_journal_file_or_directory_is_never_overwritten() {
    for is_directory in [false, true] {
        for suffix in ["plan.json", "journal.jsonl"] {
            let root = io_root("preexisting");
            let directory = root.join(".local-tests/mysql/fixtures");
            fs::create_dir_all(&directory).unwrap();
            let existing = directory.join(format!("{}.{suffix}", plan().schema));
            if is_directory {
                fs::create_dir(&existing).unwrap();
                fs::write(existing.join("retained"), b"existing-directory-evidence").unwrap();
            } else {
                fs::write(&existing, b"existing-file-evidence").unwrap();
            }
            let identity = Handle::from_path(&existing).unwrap();
            assert!(Evidence::create(&root, &plan()).is_err());
            assert_eq!(identity, Handle::from_path(&existing).unwrap());
            if is_directory {
                assert!(existing.is_dir());
                assert_eq!(
                    fs::read(existing.join("retained")).unwrap(),
                    b"existing-directory-evidence"
                );
            } else {
                assert_eq!(fs::read(existing).unwrap(), b"existing-file-evidence");
            }
        }
    }
}

#[test]
fn evidence_existing_ordinary_directories_are_retained_and_file_ancestors_are_rejected() {
    let root = io_root("existing-ancestors");
    let directory = root.join(".local-tests/mysql/fixtures");
    fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("retained");
    fs::write(&marker, b"existing-directory-evidence").unwrap();
    let identity = Handle::from_path(&directory).unwrap();
    let evidence = Evidence::create(&root, &plan()).unwrap();
    drop(evidence);
    assert_eq!(identity, Handle::from_path(&directory).unwrap());
    assert_eq!(fs::read(marker).unwrap(), b"existing-directory-evidence");

    let blocked_root = io_root("file-ancestor");
    let blocked_path = blocked_root.join(".local-tests");
    fs::write(&blocked_path, b"existing-file-ancestor").unwrap();
    let identity = Handle::from_path(&blocked_path).unwrap();
    assert!(Evidence::create(&blocked_root, &plan()).is_err());
    assert_eq!(identity, Handle::from_path(&blocked_path).unwrap());
    assert_eq!(fs::read(blocked_path).unwrap(), b"existing-file-ancestor");
    assert_eq!(fs::read_dir(blocked_root).unwrap().count(), 1);
}

#[test]
fn evidence_internal_expected_length_or_content_mismatch_rejects_both_files() {
    for journal in [false, true] {
        for length_change in [false, true] {
            let root = io_root("expected");
            let mut evidence = Evidence::create(&root, &plan()).unwrap();
            let file = if journal {
                &mut evidence.journal
            } else {
                &mut evidence.plan
            };
            let original = file.expected.clone();
            if length_change {
                file.expected.push(b'x');
            } else {
                file.expected[0] ^= 1;
            }
            assert!(evidence.verify().is_err());
            assert!(evidence.record("must-not-append").is_err());
            let file = if journal {
                &mut evidence.journal
            } else {
                &mut evidence.plan
            };
            assert_eq!(held_bytes(file), original);
        }
    }
}

#[test]
fn evidence_distinct_plan_or_journal_identity_is_rejected_even_with_equal_bytes() {
    for journal in [false, true] {
        let root = io_root("file-identity");
        let mut evidence = Evidence::create(&root, &plan()).unwrap();
        let file = if journal {
            &mut evidence.journal
        } else {
            &mut evidence.plan
        };
        let original = file.expected.clone();
        let foreign_path = root.join("different-file");
        fs::write(&foreign_path, &original).unwrap();
        file.identity = Handle::from_path(&foreign_path).unwrap();
        assert!(evidence.verify().is_err());
        assert!(evidence.record("must-not-append").is_err());
        let file = if journal {
            &mut evidence.journal
        } else {
            &mut evidence.plan
        };
        assert_eq!(held_bytes(file), original);
    }
}

#[test]
fn evidence_parent_identity_mismatch_rejects_verification_and_new_records() {
    let root = io_root("parent-identity");
    let mut evidence = Evidence::create(&root, &plan()).unwrap();
    let foreign_directory = root.join("different-directory");
    fs::create_dir(&foreign_directory).unwrap();
    let plan_bytes = evidence.plan.expected.clone();
    let journal_bytes = evidence.journal.expected.clone();
    evidence.ancestors.last_mut().unwrap().1 = Handle::from_path(foreign_directory).unwrap();
    assert!(verify_ancestors(&evidence.ancestors).is_err());
    assert!(evidence.verify().is_err());
    assert!(evidence.record("must-not-append").is_err());
    assert_eq!(held_bytes(&mut evidence.plan), plan_bytes);
    assert_eq!(held_bytes(&mut evidence.journal), journal_bytes);
}

#[test]
fn evidence_exclusive_file_locks_contend_and_release_for_both_files() {
    let root = io_root("file-lock");
    let mut evidence = Evidence::create(&root, &plan()).unwrap();
    let plan_bytes = evidence.plan.expected.clone();
    let journal_bytes = evidence.journal.expected.clone();
    assert_contended(&evidence.plan);
    assert_contended(&evidence.journal);
    assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
    let paths = [evidence.plan.path.clone(), evidence.journal.path.clone()];
    drop(evidence);
    for path in paths {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.try_lock().unwrap();
    }
}

#[test]
fn evidence_external_write_is_blocked_or_full_content_verification_rejects_it() {
    for journal in [false, true] {
        let root = io_root("external-write");
        let mut evidence = Evidence::create(&root, &plan()).unwrap();
        let plan_bytes = evidence.plan.expected.clone();
        let journal_bytes = evidence.journal.expected.clone();
        let path = if journal {
            &evidence.journal.path
        } else {
            &evidence.plan.path
        };
        let written = OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|mut other| {
                let count = other.write(b"!")?;
                other.sync_all()?;
                Ok(count)
            });
        match written {
            Ok(count) => {
                assert_eq!(count, 1);
                assert!(evidence.verify().is_err());
                assert!(evidence.record("must-not-append").is_err());
                let file = if journal {
                    &mut evidence.journal
                } else {
                    &mut evidence.plan
                };
                let mut changed = file.expected.clone();
                changed[0] = b'!';
                assert_eq!(held_bytes(file), changed);
            }
            Err(error) => {
                assert_windows_lock_denial(&error);
                assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
                eprintln!("Windows 文件锁保护实际生效；原计划与 journal 的身份和完整内容未变");
            }
        }
    }
}

#[test]
fn evidence_external_replacement_is_blocked_or_distinct_path_identity_is_rejected() {
    for journal in [false, true] {
        let root = io_root("external-replace");
        let mut evidence = Evidence::create(&root, &plan()).unwrap();
        let plan_bytes = evidence.plan.expected.clone();
        let journal_bytes = evidence.journal.expected.clone();
        let file = if journal {
            &evidence.journal
        } else {
            &evidence.plan
        };
        let replacement = root.join("replacement");
        fs::write(&replacement, &file.expected).unwrap();
        match fs::rename(&replacement, &file.path) {
            Ok(()) => {
                let file = if journal {
                    &evidence.journal
                } else {
                    &evidence.plan
                };
                assert_ne!(file.identity, Handle::from_path(&file.path).unwrap());
                assert_eq!(fs::read(&file.path).unwrap(), file.expected);
                assert!(evidence.verify().is_err());
                assert!(evidence.record("must-not-append").is_err());
                assert_eq!(held_bytes(&mut evidence.plan), plan_bytes);
                assert_eq!(held_bytes(&mut evidence.journal), journal_bytes);
            }
            Err(error) => {
                assert_windows_lock_denial(&error);
                assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
                assert_eq!(
                    fs::read(&replacement).unwrap(),
                    file_expected(&evidence, journal)
                );
                eprintln!("Windows 持有句柄保护实际生效；替换被拒绝且原证据未变");
            }
        }
    }
}

fn file_expected(evidence: &Evidence, journal: bool) -> &[u8] {
    if journal {
        &evidence.journal.expected
    } else {
        &evidence.plan.expected
    }
}

#[test]
fn evidence_parent_path_replacement_is_blocked_or_detected_before_further_creation() {
    let root = io_root("parent-replace");
    let mut evidence = Evidence::create(&root, &plan()).unwrap();
    let directory = evidence.ancestors.last().unwrap().0.clone();
    let retained = root.join("retained-fixtures");
    let plan_bytes = evidence.plan.expected.clone();
    let journal_bytes = evidence.journal.expected.clone();
    match fs::rename(&directory, &retained) {
        Ok(()) => {
            fs::create_dir(&directory).unwrap();
            assert!(verify_ancestors(&evidence.ancestors).is_err());
            assert!(evidence.verify().is_err());
            assert!(evidence.record("must-not-append").is_err());
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
            assert!(retained.is_dir());
        }
        Err(error) => {
            assert_windows_lock_denial(&error);
            assert_unchanged(&mut evidence, &plan_bytes, &journal_bytes);
            assert!(!retained.exists());
            eprintln!("Windows 目录身份句柄保护实际生效；目录替换被拒绝且原证据未变");
        }
    }
    assert_eq!(held_bytes(&mut evidence.plan), plan_bytes);
    assert_eq!(held_bytes(&mut evidence.journal), journal_bytes);
}

#[test]
fn business_connection_matching_server_uuid_and_schema_is_accepted() {
    let expected = plan();
    assert!(business_identity_matches(
        &expected.identity.server_uuid,
        &expected.schema,
        &expected.identity.server_uuid,
        Some(&expected.schema),
    ));
}

#[test]
fn business_connection_different_server_uuid_is_rejected_with_the_same_schema() {
    let expected = plan();
    assert!(!business_identity_matches(
        &expected.identity.server_uuid,
        &expected.schema,
        "20000000-0000-4000-8000-000000000002",
        Some(&expected.schema),
    ));
}

#[test]
fn business_connection_different_schema_is_rejected_on_the_same_server() {
    let expected = plan();
    assert!(!business_identity_matches(
        &expected.identity.server_uuid,
        &expected.schema,
        &expected.identity.server_uuid,
        Some("ryframe_it_another_schema"),
    ));
}

#[test]
fn business_connection_null_database_is_rejected_on_the_same_server() {
    let expected = plan();
    assert!(!business_identity_matches(
        &expected.identity.server_uuid,
        &expected.schema,
        &expected.identity.server_uuid,
        None,
    ));
}

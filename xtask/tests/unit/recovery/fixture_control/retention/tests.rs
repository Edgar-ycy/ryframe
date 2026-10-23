use super::*;
use crate::devex::job_evidence::protocol::{EvidenceResult, Session};
use std::{collections::VecDeque, fs, path::Path};

const TENANT: &str = "tenant-0123abcd";
const MIGRATION: i64 = 123456789;
fn snapshot() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../src/recovery/fixture_control/retention/snapshot.json"
    ))
    .unwrap()
}
fn contract() -> Value {
    json!({"scope_id":"history-test","tenant_id":TENANT,"migration_id":MIGRATION.to_string(),"server_uuid":"server","schemas":["control_test","source_test","target_test"],"configuration_sha256":"a".repeat(64)})
}
fn temporary() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn shifted(before: &Value) -> Value {
    let mut after = before.clone();
    after["remaining_seconds"] = json!(before["remaining_seconds"].as_i64().unwrap() - 169 * 3600);
    let shift = |value: &mut Value| {
        for time in value.as_object_mut().unwrap().values_mut() {
            if !time.is_null() {
                let instant = chrono::DateTime::parse_from_rfc3339(time.as_str().unwrap()).unwrap()
                    - chrono::Duration::hours(169);
                *time = json!(instant.to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
            }
        }
    };
    for field in ["migration_times", "item_times", "job_times"] {
        shift(&mut after[field]);
    }
    for attempt in after["attempt_records"].as_array_mut().unwrap() {
        shift(&mut attempt["times"]);
    }
    after
}
fn retried() -> Value {
    let mut row = snapshot();
    let success = row["attempt_records"][0].clone();
    let mut failed = success.clone();
    failed["outcome"] = json!("failed");
    let mut expired = success.clone();
    expired["sequence"] = json!("2");
    expired["outcome"] = json!("lease_expired");
    expired["times"]["finished_at"] = Value::Null;
    let mut success = success;
    success["sequence"] = json!("3");
    row["job_claim_sequence"] = json!("3");
    row["attempt_records"] = json!([failed, expired, success]);
    row
}
#[derive(Default)]
struct Database {
    responses: VecDeque<EvidenceResult<Vec<Value>>>,
    statements: Vec<String>,
}
impl Database {
    fn rows(values: Vec<Vec<Value>>) -> Self {
        Self {
            responses: values.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }
}
impl Session for Database {
    fn query(&mut self, statement: &str) -> EvidenceResult<Vec<Value>> {
        self.statements.push(statement.into());
        self.responses.pop_front().expect("未预期 SQL")
    }
    fn close(&mut self) -> EvidenceResult<()> {
        Ok(())
    }
}
struct Export {
    fail: bool,
    called: bool,
}
impl backup::Exporter for Export {
    fn export(&mut self, directory: &Path, migration: i64) -> Result<Value> {
        self.called = true;
        if self.fail {
            return Err("导出失败".into());
        }
        let path = directory.join(format!("business-retention-{migration}.sql"));
        fs::write(&path, b"actual exported content").unwrap();
        super::super::file_binding(&path, "测试导出")
    }
}
fn history(directory: &Path, before: &Value) {
    let mut receipt = contract();
    receipt["artificial_history"] = json!(true);
    receipt["before"] = before.clone();
    backup::publish(
        &directory.join(format!("business-history-{MIGRATION}.json")),
        &receipt,
    )
    .unwrap();
}

#[test]
fn plan_reads_once_without_writes_and_binds_every_attempt() {
    let directory = temporary();
    let row = retried();
    let mut db = Database::rows(vec![vec![row.clone()]]);
    let plan = engine::plan(&mut db, "snapshot", &contract()).unwrap();
    assert_eq!(db.statements, ["snapshot"]);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    let mut later = row.clone();
    later["remaining_seconds"] = json!(row["remaining_seconds"].as_i64().unwrap() - 10);
    later["attempt_records"].as_array_mut().unwrap().reverse();
    assert_eq!(
        model::plan(&contract(), later).unwrap()["plan_sha256"],
        plan["plan_sha256"]
    );
    for field in ["scope_id", "server_uuid"] {
        let mut changed = contract();
        changed[field] = json!("other");
        assert_ne!(
            model::plan(&changed, row.clone()).unwrap()["plan_sha256"],
            plan["plan_sha256"]
        );
    }
    let mut foreign = contract();
    foreign["tenant_id"] = json!("other");
    assert!(model::plan(&foreign, row).is_err());
}

#[test]
fn plan_digest_preserves_existing_compact_json_byte_contract() {
    assert_eq!(
        model::plan(&contract(), snapshot()).unwrap()["plan_sha256"],
        "e1117803f66dae3d45050af9503ee14eb2d542974dcf0a489b9c0bb805d11bb2"
    );
}

#[test]
fn complete_shift_preserves_state_policy_and_unknown_finish() {
    let directory = temporary();
    let row = retried();
    let after = shifted(&row);
    let hash = model::plan(&contract(), row.clone()).unwrap()["plan_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut db = Database::rows(vec![vec![row.clone()], vec![], vec![after.clone()], vec![]]);
    let result = engine::apply(
        &mut db,
        "control_test",
        "snapshot",
        directory.path(),
        &contract(),
        &hash,
    )
    .unwrap();
    assert_eq!(result["state"], "historical-expired");
    assert_eq!(result["retention_hours"], 168);
    assert_eq!(result["natural_elapsed_7_days"], false);
    assert!(result["after"]["attempt_records"][1]["times"]["finished_at"].is_null());
    assert_eq!(db.statements.last().unwrap(), "COMMIT");
    assert!(!db.statements[1].contains("retention_hours="));
    assert!(!db.statements[1].contains("state="));
    assert!(db.statements[1].contains("a.sequence IN (1,2,3)"));
    let evidence = super::super::read_json(
        &directory
            .path()
            .join(format!("business-history-{MIGRATION}.json")),
        "前像",
        4 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(evidence["before"], row);
    let mut repeated = Database::rows(vec![vec![row]]);
    assert!(
        engine::apply(
            &mut repeated,
            "control_test",
            "snapshot",
            directory.path(),
            &contract(),
            &hash
        )
        .is_err()
    );
    assert_eq!(repeated.statements.len(), 1);
}

#[test]
fn partial_shift_and_unknown_write_never_commit_and_keep_front_image() {
    for interrupted in [false, true] {
        let directory = temporary();
        let before = snapshot();
        let mut after = shifted(&before);
        after["item_times"]["copied_at"] = before["item_times"]["copied_at"].clone();
        let hash = model::plan(&contract(), before.clone()).unwrap()["plan_sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut db = Database::rows(vec![vec![before], vec![], vec![after]]);
        if interrupted {
            db.responses[1] = Err("mysql-eof");
        }
        assert!(
            engine::apply(
                &mut db,
                "control_test",
                "snapshot",
                directory.path(),
                &contract(),
                &hash
            )
            .is_err()
        );
        assert!(!db.statements.iter().any(|s| s == "COMMIT"));
        assert!(
            directory
                .path()
                .join(format!("business-history-{MIGRATION}.json"))
                .is_file()
        );
    }
}

#[test]
fn lost_commit_confirmation_preserves_receipt_without_fabricating_completion() {
    let directory = temporary();
    let row = snapshot();
    let after = shifted(&row);
    let hash = model::plan(&contract(), row.clone()).unwrap()["plan_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut db = Database::rows(vec![vec![row], vec![], vec![after], vec![]]);
    db.responses[3] = Err("mysql-eof");
    let error = engine::apply(
        &mut db,
        "control_test",
        "snapshot",
        directory.path(),
        &contract(),
        &hash,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("可能未知"));
    assert!(error.contains("禁止自动重放"));
    assert!(
        directory
            .path()
            .join(format!("business-history-{MIGRATION}.json"))
            .is_file()
    );
    assert_eq!(
        db.statements.iter().filter(|sql| *sql == "COMMIT").count(),
        1
    );
}

#[test]
fn running_worker_foreign_artifact_and_reused_pid_block_before_database_access() {
    let directory = temporary();
    let identity = crate::recovery::clone::native::lifecycle::identity::current().unwrap();
    let runtime =
        json!({"scope_id":"history-test","artifacts":{"worker":{"path":identity["executable"]}}});
    let path = directory.path().join("worker.json");
    let receipt =
        json!({"format_version":1,"role":"worker","scope_id":"history-test","identity":identity});
    for mode in 0..3 {
        let mut value = receipt.clone();
        if mode == 1 {
            value["identity"]["executable"] = json!("wrong-artifact");
        }
        if mode == 2 {
            value["identity"]["started"] = json!("1");
        }
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        super::super::write_new(&path, &value).unwrap();
        assert!(require_worker_stopped(directory.path(), &runtime).is_err());
    }
}

#[test]
fn drift_or_natural_expiry_fails_before_creating_evidence() {
    let directory = temporary();
    let before = retried();
    let mut changed = before.clone();
    changed["attempt_records"][0]["times"]["available_at"] = json!("2026-09-03T00:00:00.000000Z");
    let hash = model::plan(&contract(), before).unwrap()["plan_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    for row in [changed, {
        let mut row = snapshot();
        row["remaining_seconds"] = json!(-1);
        row
    }] {
        let mut db = Database::rows(vec![vec![row]]);
        assert!(
            engine::apply(
                &mut db,
                "control_test",
                "snapshot",
                directory.path(),
                &contract(),
                &hash
            )
            .is_err()
        );
        assert_eq!(db.statements.len(), 1);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn ownership_and_cleanup_checks_reject_foreign_or_partial_records() {
    let mutations = json!({"target_mode":"shared","job_tenant_id":"other","job_migration_id":"999","job_status":"running","job_lease_clear":0,"target_slot_matches":0,"source_fence_matches":0,"source_slot_matches":0,"target_total":4,"other_migrations":1,"operation_leases":1,"retention_hours":1,"cleanup_state":"cleaned","job_id":"1 OR 1=1"});
    for (key, value) in mutations.as_object().unwrap() {
        let mut row = snapshot();
        row[key] = value.clone();
        assert!(
            model::validate(vec![row], TENANT, MIGRATION, false).is_err(),
            "{key}"
        );
    }
    let before = snapshot();
    let mut changed = shifted(&before);
    changed["fingerprint"] = json!("d".repeat(64));
    assert!(model::verify_shift(&before, &changed).is_err());
    let mut row = shifted(&before);
    for (key,value) in json!({"state":"finalized","intent_clear":0,"cleanup_state":"cleaned","cleanup_count":3,"source_count":0,"source_total":0,"source_fence_matches":0,"source_slot_matches":0,"source_fences":0,"source_slots":0}).as_object().unwrap(){row[key]=value.clone();}
    model::validate(vec![row.clone()], TENANT, MIGRATION, true).unwrap();
    for (key, value) in
        json!({"target_count":0,"source_fences":1,"source_slots":1,"cleanup_count":2})
            .as_object()
            .unwrap()
    {
        let mut changed = row.clone();
        changed[key] = value.clone();
        assert!(model::validate(vec![changed], TENANT, MIGRATION, true).is_err());
    }
}

#[test]
fn attempt_ownership_sequence_and_closed_time_are_all_required() {
    for mode in 0..9 {
        let mut row = retried();
        match mode {
            0 => {
                row["attempt_records"].as_array_mut().unwrap().pop();
            }
            1 => row["attempt_records"][0]["job_id"] = json!("999"),
            2 => row["attempt_records"][0]["sequence"] = json!("2"),
            3 => row["attempt_records"][0]["outcome"] = json!("running"),
            4 => row["job_claim_sequence"] = json!("4"),
            5 => row["attempt_records"][0]["times"]["finished_at"] = Value::Null,
            6 => {
                row["attempt_records"][1]["times"]["finished_at"] =
                    row["job_times"]["completed_at"].clone()
            }
            7 => row["attempt_records"][0]["times"]["started_at"] = json!("2020-01-01T00:00:00Z"),
            _ => row["attempt_records"][2]["outcome"] = json!("failed"),
        }
        assert!(
            model::validate(vec![row], TENANT, MIGRATION, false).is_err(),
            "{mode}"
        );
    }
}

#[test]
fn backup_failures_and_missing_fence_never_register_or_commit() {
    let directory = temporary();
    let before = snapshot();
    history(directory.path(), &before);
    let row = shifted(&before);
    for fence_missing in [true, false] {
        let mut db = Database::rows(vec![
            vec![row.clone()],
            vec![json!({"count":0})],
            if fence_missing {
                vec![]
            } else {
                vec![json!({"tenant_id":TENANT,"generation":"2"})]
            },
            vec![],
        ]);
        let mut export = Export {
            fail: true,
            called: false,
        };
        assert!(
            engine::export(
                &mut db,
                &mut export,
                "control_test",
                "target_test",
                "snapshot",
                directory.path(),
                &contract()
            )
            .is_err()
        );
        assert_eq!(export.called, !fence_missing);
        assert!(
            !db.statements
                .iter()
                .any(|s| s.starts_with("INSERT") || s == "COMMIT")
        );
    }
}

#[test]
fn successful_backup_binds_fence_capture_digest_and_cannot_claim_restore() {
    let directory = temporary();
    let before = snapshot();
    history(directory.path(), &before);
    let row = shifted(&before);
    let mut db = Database::rows(vec![
        vec![row.clone()],
        vec![json!({"count":0})],
        vec![json!({"tenant_id":TENANT,"generation":"2"})],
        vec![],
        vec![row],
        vec![],
        vec![],
    ]);
    let mut export = Export {
        fail: false,
        called: false,
    };
    let evidence = engine::export(
        &mut db,
        &mut export,
        "control_test",
        "target_test",
        "snapshot",
        directory.path(),
        &contract(),
    )
    .unwrap();
    assert_eq!(evidence["restored"], false);
    assert_eq!(evidence["tenant_write_fence_held"], true);
    assert_eq!(db.statements.last().unwrap(), "COMMIT");
    assert!(db.statements[5].contains("@backup_captured_at"));
    assert!(db.statements[5].contains("m.target_generation"));
    assert!(db.statements[5].contains("未执行恢复验证"));
    backup::verify(&evidence["artifact"], directory.path(), MIGRATION).unwrap();
    fs::write(
        directory
            .path()
            .join(format!("business-retention-{MIGRATION}.sql")),
        b"corrupt",
    )
    .unwrap();
    assert!(backup::verify(&evidence["artifact"], directory.path(), MIGRATION).is_err());
}

#[test]
fn dump_requires_structure_and_exact_three_independent_inserts() {
    assert!(backup::validate_dump(b"incomplete").is_err());
    let text = format!(
        "CREATE TABLE `biz_order` ();\n{}",
        "INSERT INTO `biz_order` VALUES (1);\n".repeat(3)
    );
    backup::validate_dump(text.as_bytes()).unwrap();
    assert!(
        backup::validate_dump(format!("{text}INSERT INTO `biz_order` VALUES (4);").as_bytes())
            .is_err()
    );
}

#[test]
fn locks_bind_all_attempts_to_tenant_and_migration() {
    let mut db = Database::rows(vec![
        vec![],
        vec![json!({"id":MIGRATION.to_string()})],
        vec![],
        vec![],
        vec![],
    ]);
    engine::lock_records(&mut db, "control_test", &contract()).unwrap();
    let sql = db.statements.last().unwrap();
    for part in [
        "sys_background_job_attempt a",
        "m.tenant_id='tenant-0123abcd'",
        "j.tenant_id='tenant-0123abcd'",
        "ORDER BY a.sequence FOR UPDATE",
    ] {
        assert!(sql.contains(part));
    }
    assert!(
        sql::snapshot(
            &["bad`schema".into(), "source".into(), "target".into()],
            TENANT,
            MIGRATION
        )
        .is_err()
    );
    assert!(validate_selection("tenant-0123ABCD", 1).is_err());
}

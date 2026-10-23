use super::*;
use crate::devex::job_evidence::protocol::EvidenceResult;
use std::{collections::VecDeque, io::Cursor};
const TENANT: &str = "tenant-0123abcd";
const MIGRATION: i64 = 123456789;
#[derive(Default)]
struct Database {
    responses: VecDeque<EvidenceResult<Vec<Value>>>,
    statements: Vec<String>,
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
fn pending() -> Value {
    json!({"migration_id":MIGRATION.to_string(),"tenant_id":TENANT,"source":"shared-control","target":"shared","state":"prechecking","retention_hours":168,"job_status":"pending","attempts":0,"job_migration_id":MIGRATION.to_string()})
}
fn databases() -> (Database, Database) {
    (
        Database {
            responses: vec![
                Ok(vec![pending()]),
                Ok(vec![json!({"source":3,"target":0})]),
                Ok(vec![json!({"waiting_connection_id":81})]),
            ]
            .into(),
            ..Default::default()
        },
        Database {
            responses: vec![Ok(vec![json!({"connection_id":42})]), Ok(vec![])].into(),
            ..Default::default()
        },
    )
}
fn invoke(observer: &mut Database, gate: &mut Database, input: &str) -> Result<Vec<Value>> {
    let contract = json!({"scope_id":"migration-gate-test","tenant_id":TENANT,"migration_id":MIGRATION.to_string()});
    let mut output = Vec::new();
    hold(
        observer,
        gate,
        Selection {
            control: "control_test",
            target: "shared_test",
            contract: &contract,
            timeout: Duration::ZERO,
        },
        &mut Cursor::new(input),
        &mut output,
    )?;
    Ok(String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect())
}
#[test]
fn gate_proves_actual_wait_and_releases_exact_lock_transaction() {
    let (mut observer, mut gate) = databases();
    let output = invoke(
        &mut observer,
        &mut gate,
        "{\"operation\":\"wait-blocked\"}\n{\"operation\":\"release\"}\n",
    )
    .unwrap();
    assert_eq!(
        output
            .iter()
            .map(|row| row["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["held", "blocked", "released"]
    );
    assert_eq!(output[1]["proof"]["waiting_connection_id"], 81);
    assert_eq!(gate.statements.last().unwrap(), "ROLLBACK");
    assert!(
        gate.statements[0]
            .contains("`shared_test`.biz_order WHERE tenant_id='tenant-0123abcd' FOR UPDATE")
    );
    for required in [
        "b.PROCESSLIST_ID=42",
        "l.OBJECT_SCHEMA='shared_test'",
        "m.id=123456789 AND m.tenant_id='tenant-0123abcd'",
        "i.table_name='biz_order'",
        "m.state='copying'",
        "i.state='copying'",
        "j.status='running'",
    ] {
        assert!(observer.statements.last().unwrap().contains(required));
    }
}
#[test]
fn foreign_pending_or_existing_target_data_never_acquires_lock() {
    for (field,value) in json!({"state":"copying","attempts":1,"job_migration_id":"999","retention_hours":1,"target":"dedicated-a"}).as_object().unwrap(){let mut row=pending();row[field]=value.clone();assert!(validate_pending(vec![row],TENANT,MIGRATION).is_err());}
    let (mut observer, mut gate) = databases();
    observer.responses[1] = Ok(vec![json!({"source":3,"target":1})]);
    assert!(invoke(&mut observer, &mut gate, "").is_err());
    assert!(gate.statements.is_empty());
}
#[test]
fn ambiguous_wait_timeout_and_connection_crash_cannot_claim_release() {
    for response in [Ok(vec![json!({}), json!({})]), Ok(vec![]), Err("mysql-eof")] {
        let (mut observer, mut gate) = databases();
        observer.responses[2] = response;
        assert!(
            invoke(
                &mut observer,
                &mut gate,
                "{\"operation\":\"wait-blocked\"}\n"
            )
            .is_err()
        );
        assert!(!gate.statements.iter().any(|s| s == "COMMIT"));
    }
}
#[test]
fn eof_and_invalid_commands_never_mutate_migration_data() {
    let (mut observer, mut gate) = databases();
    let output = invoke(&mut observer, &mut gate, "").unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0]["state"], "held");
    for input in [
        "{\"operation\":\"delete\"}\n",
        "{\"operation\":\"release\",\"operation\":\"delete\"}\n",
        "{\"operation\":\"release\",\"unknown\":1}\n",
    ] {
        let (mut observer, mut gate) = databases();
        assert!(invoke(&mut observer, &mut gate, input).is_err());
        for statement in observer.statements.iter().chain(&gate.statements) {
            assert!(!statement.starts_with("UPDATE "));
            assert!(!statement.starts_with("DELETE "));
            assert!(!statement.starts_with("INSERT "));
        }
    }
    assert!(read_command(&mut Cursor::new(vec![b'x'; 16 * 1024 + 1])).is_err());
}

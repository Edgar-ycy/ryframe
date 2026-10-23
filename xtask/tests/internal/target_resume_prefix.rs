use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use crate::{
    recovery::clone::native::{
        digest,
        target_verify::{evidence_protocol, resume_prefix},
    },
    workspace,
};

struct Fixture {
    backend: PathBuf,
    directory: tempfile::TempDir,
    operations: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let parent = backend.join(".local-tests/target-resume-prefix");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let operations =
            resume_prefix::operations("fixture-migrate.exe").unwrap()["operations"].clone();
        Self {
            backend,
            directory,
            operations,
        }
    }

    fn path(&self, index: usize, attempt: u32) -> PathBuf {
        let stage = self.operations[index]["stage"].as_str().unwrap();
        self.directory
            .path()
            .join(format!("{stage}-{attempt:032x}.command.json"))
    }

    fn receipt(&self, index: usize) -> Value {
        json!({"command":self.operations[index]["command"],"stdout":self.operations[index]["stdout"],
            "returncode":0,"error_type":null,"stderr":""})
    }

    fn write(&self, index: usize) {
        fs::write(
            self.path(index, 0),
            serde_json::to_vec(&self.receipt(index)).unwrap(),
        )
        .unwrap();
    }

    fn prefix(&self, complete: bool) -> crate::Result<Value> {
        resume_prefix::prefix(
            &self.backend,
            self.directory.path(),
            &self.operations,
            complete,
        )
    }
}

#[test]
fn sequence_matches_existing_ten_operations_and_exact_platform_output() {
    let fixture = Fixture::new();
    let operations = fixture.operations.as_array().unwrap();
    assert_eq!(operations.len(), 10);
    for (index, target) in [
        None,
        Some("dedicated-a"),
        Some("dedicated-b"),
        Some("shared"),
        Some("shared-control"),
    ]
    .into_iter()
    .enumerate()
    {
        let scope = if target.is_some() {
            "tenant-data"
        } else {
            "control"
        };
        for (offset, action) in ["up", "verify"].into_iter().enumerate() {
            let item = &operations[index * 2 + offset];
            let mut command = vec!["fixture-migrate.exe", scope, action];
            let stem = if let Some(target) = target {
                command.extend(["--target", target]);
                format!("{scope}---target-{target}")
            } else {
                scope.into()
            };
            let stdout = format!(
                "scope={scope} {}{}",
                target
                    .map(|value| format!("target={value} "))
                    .unwrap_or_default(),
                if offset == 0 {
                    "migration=completed schema=verified\n"
                } else {
                    "migration_ledger=current schema=verified\n"
                }
            );
            assert_eq!(item["command"], json!(command));
            assert_eq!(item["stage"], format!("migrate-{stem}-{action}"));
            assert_eq!(item["write"], offset == 0);
            assert_eq!(item["stdout"], stdout);
        }
    }
}

#[test]
fn every_boundary_requires_missing_readonly_verify_or_explicit_completion() {
    for count in 0..=10 {
        let fixture = Fixture::new();
        for index in 0..count {
            fixture.write(index);
        }
        assert_eq!(
            fixture.prefix(false).is_ok(),
            count % 2 == 1,
            "prefix {count}"
        );
        assert_eq!(
            fixture.prefix(true).is_ok(),
            count == 10,
            "complete {count}"
        );
    }
}

#[test]
fn prefix_refuses_holes_duplicates_unknown_names_and_modified_operation_sequence() {
    let fixture = Fixture::new();
    fixture.write(0);
    fixture.write(2);
    assert!(fixture.prefix(false).is_err());
    fs::remove_file(fixture.path(2, 0)).unwrap();
    fs::copy(fixture.path(0, 0), fixture.path(0, 1)).unwrap();
    assert!(
        fixture
            .prefix(false)
            .unwrap_err()
            .to_string()
            .contains("重复命令收据")
    );
    fs::remove_file(fixture.path(0, 1)).unwrap();
    let unknown = fixture
        .directory
        .path()
        .join("migrate-unknown.command.json");
    fs::write(&unknown, b"{}").unwrap();
    assert!(
        fixture
            .prefix(false)
            .unwrap_err()
            .to_string()
            .contains("未知命令收据")
    );
    fs::remove_file(unknown).unwrap();
    let mut changed = fixture.operations.clone();
    changed[0]["write"] = json!(false);
    assert!(
        resume_prefix::prefix(&fixture.backend, fixture.directory.path(), &changed, false).is_err()
    );
}

#[test]
fn command_receipt_requires_exact_command_status_and_full_output() {
    for case in 0..9 {
        let fixture = Fixture::new();
        let mut value = fixture.receipt(0);
        match case {
            0 => value["returncode"] = json!(true),
            1 => value["returncode"] = json!(1),
            2 => value["stderr"] = json!("diagnostic"),
            3 => value["error_type"] = json!("TimeoutExpired"),
            4 => value["command"] = json!(["other.exe"]),
            5 => value["stdout"] = json!("truncated"),
            6 => value["stdout"] = json!(false),
            7 => value["extra"] = json!(1),
            _ => value
                .as_object_mut()
                .unwrap()
                .remove("stderr")
                .map(|_| ())
                .unwrap(),
        }
        fs::write(fixture.path(0, 0), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(fixture.prefix(false).is_err(), "case {case}");
    }
}

#[test]
fn successful_operation_accepts_crlf_without_modifying_receipt_and_binds_bytes() {
    let fixture = Fixture::new();
    let mut value = fixture.receipt(0);
    value["stdout"] = json!(value["stdout"].as_str().unwrap().replace('\n', "\r\n"));
    let bytes = serde_json::to_vec(&value).unwrap();
    let path = fixture.path(0, 0);
    fs::write(&path, &bytes).unwrap();
    let receipt = resume_prefix::operation_record(
        &fixture.backend,
        fixture.directory.path(),
        &fixture.operations[0],
    )
    .unwrap();
    assert_eq!(
        receipt,
        json!({"path":path,"bytes":bytes.len(),"sha256":digest::file(&path).unwrap().sha256})
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let request = json!({"format_version":1,"operation":"migration-operation-record",
        "backend":fixture.backend,"target":fixture.directory.path(),
        "migration_operation":fixture.operations[0]});
    assert_eq!(
        evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).unwrap(),
        receipt
    );
    assert!(fixture.prefix(false).is_ok());
    fs::copy(&path, fixture.path(0, 1)).unwrap();
    assert!(
        resume_prefix::operation_record(
            &fixture.backend,
            fixture.directory.path(),
            &fixture.operations[0]
        )
        .is_err()
    );
}

#[test]
fn protocol_requires_boolean_complete_and_rejects_duplicate_receipt_fields() {
    let fixture = Fixture::new();
    fixture.write(0);
    let mut request = json!({"format_version":1,"operation":"migration-prefix","backend":fixture.backend,
        "target":fixture.directory.path(),"operations":fixture.operations,"complete":false});
    let result = evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(result["next_index"], 1);
    request["complete"] = json!(0);
    assert!(evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).is_err());
    fs::write(fixture.path(0, 0), b"{\"command\":[],\"command\":[]}").unwrap();
    assert!(fixture.prefix(false).is_err());
}

use crate::{
    Result,
    recovery::clone::native::{
        export::transport::Transport,
        json as document,
        target_verify::{
            fresh::{initialized_objects, objects, resources::database_absence},
            input::request_physical,
            storage::StorageInputs,
        },
    },
};
use crate::{
    recovery::clone::native::{
        export::transport::{CommandOutput, CommandSpec},
        s3::{Operation, Request},
    },
    target_verify_fixture::Fixture,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;

struct ReadOnly {
    mysql: String,
    defaults_file: String,
    object_case: &'static str,
    calls: RefCell<Vec<String>>,
}
impl Transport for ReadOnly {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let defaults = spec.args[0].strip_prefix("--defaults-file=").unwrap();
        assert_eq!(defaults, self.defaults_file);
        let sql = String::from_utf8(spec.stdin.unwrap()).unwrap();
        assert!(sql.starts_with("SELECT @@server_uuid; SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '"));
        assert!(!sql.contains("CREATE") && !sql.contains("DROP"));
        self.calls.borrow_mut().push(sql);
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(0)
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(0)
        };
        Ok(CommandOutput {
            status,
            stdout: self.mysql.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }
    fn s3(&self, request: Request) -> Result<Value> {
        self.calls.borrow_mut().push(request.bucket.clone());
        if self.object_case == "denied" {
            return Err("403 fixture-access fixture-secret".into());
        }
        match request.operation {
            Operation::Versioning if self.object_case == "versioned" => {
                Ok(json!({"Status":"Enabled"}))
            }
            Operation::HeadBucket | Operation::Versioning => Ok(json!({})),
            Operation::Get {
                key,
                destination,
                max_bytes,
                if_match,
                version,
            } => {
                assert!(matches!(
                    self.object_case,
                    "initialized" | "wrong-owner" | "wrong-metadata"
                ));
                assert_eq!(key, "seed-scope/.ryframe-owner");
                assert_eq!(max_bytes, 4096);
                assert!(if_match.is_none() && version.is_none());
                let owner = if self.object_case == "wrong-owner" {
                    "foreign-owner".to_owned()
                } else {
                    format!(
                        "ryframe-owner:v1:seed-scope:object-storage:{}",
                        request.bucket
                    )
                };
                std::fs::write(destination, owner.as_bytes()).unwrap();
                Ok(
                    json!({"ContentLength":owner.len(),"ETag":"\"owner\"","ContentType":"text/plain","LastModified":"2026-10-26T00:00:00Z","Metadata":if self.object_case=="wrong-metadata" {json!(false)}else{json!({})}}),
                )
            }
            Operation::List {
                prefix,
                continuation,
            } => {
                assert_eq!(prefix, "seed-scope/");
                assert!(continuation.is_none());
                Ok(json!({"IsTruncated":self.object_case == "truncated",
                    "Contents":if self.object_case == "nonempty" {json!([{"Key":"seed-scope/owned"}])} else if matches!(self.object_case,"initialized"|"wrong-owner"|"wrong-metadata") {json!([{"Key":"seed-scope/.ryframe-owner"}])} else {json!([])}}))
            }
            _ => panic!("prepare 不得写对象或读取 owner"),
        }
    }
}

#[test]
fn four_database_absence_checks_require_the_exact_uuid_and_do_not_issue_write_sql() {
    let f = Fixture::for_status();
    let mut environment = f.environment.clone();
    environment.insert("TEMP".into(), f.local.to_str().unwrap().into());
    let secret =
        request_physical::defaults_connection(&f.backend, &f.request["target"]["databases"][0])
            .unwrap()["password"]
            .as_str()
            .unwrap()
            .to_owned();
    for uuid in [
        crate::target_verify_fixture::UUID,
        "wrong-server",
        secret.as_str(),
    ] {
        let transport = ReadOnly {
            mysql: format!("{uuid}\r\n"),
            defaults_file: f.request["target"]["databases"][0]["defaults_file"]
                .as_str()
                .unwrap()
                .to_owned(),
            object_case: "empty",
            calls: RefCell::new(Vec::new()),
        };
        let inputs = StorageInputs {
            backend: &f.backend,
            request: &f.request,
            environment: &environment,
            observation_dir: &f.local,
        };
        for db in f.request["target"]["databases"].as_array().unwrap() {
            let result = database_absence(&inputs, &transport, db);
            if uuid == crate::target_verify_fixture::UUID {
                assert_eq!(result.unwrap()["exists"], false);
            } else {
                assert!(result.is_err());
            }
        }
        assert_eq!(transport.calls.borrow().len(), 4);
        let receipts = std::fs::read_dir(&f.local)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("mysql-")
                    .then(|| std::fs::read_to_string(path).unwrap())
            })
            .collect::<String>();
        assert!(!receipts.contains(&secret));
    }
}

#[test]
fn five_bucket_absence_is_complete_and_denied_reads_never_become_empty_or_expose_credentials() {
    let f = Fixture::for_status();
    for object_case in ["empty", "denied", "nonempty", "truncated", "versioned"] {
        let output = f.local.join(format!("object-{object_case}"));
        std::fs::create_dir(&output).unwrap();
        let transport = ReadOnly {
            mysql: String::new(),
            defaults_file: f.request["target"]["databases"][0]["defaults_file"]
                .as_str()
                .unwrap()
                .to_owned(),
            object_case,
            calls: RefCell::new(Vec::new()),
        };
        let inputs = StorageInputs {
            backend: &f.backend,
            request: &f.request,
            environment: &f.environment,
            observation_dir: &output,
        };
        let result = objects::absence(&inputs, &transport);
        if object_case != "empty" {
            assert!(result.is_err(), "{object_case}");
            assert_eq!(
                transport.calls.borrow().len(),
                match object_case {
                    "denied" => 1,
                    "versioned" => 2,
                    _ => 3,
                }
            );
        } else {
            assert_eq!(result.unwrap().as_object().unwrap().len(), 5);
            assert_eq!(transport.calls.borrow().len(), 15);
        }
        for entry in std::fs::read_dir(&output).unwrap() {
            let receipt = document::read(&entry.unwrap().path()).unwrap();
            assert_public_object_receipt(&receipt, object_case == "denied");
        }
        assert!(!output.join("prepare.json").exists());
    }
}

#[test]
fn initialized_objects_require_all_five_exact_owners_and_reject_unknown_or_changed_bodies() {
    let f = Fixture::for_status();
    for object_case in ["initialized", "wrong-owner", "wrong-metadata", "nonempty"] {
        let output = f.local.join(format!("owner-{object_case}"));
        std::fs::create_dir(&output).unwrap();
        let transport = ReadOnly {
            mysql: String::new(),
            defaults_file: String::new(),
            object_case,
            calls: RefCell::new(Vec::new()),
        };
        let inputs = StorageInputs {
            backend: &f.backend,
            request: &f.request,
            environment: &f.environment,
            observation_dir: &output,
        };
        let result = initialized_objects::observe(&inputs, &transport);
        if object_case == "initialized" {
            let value = result.unwrap();
            assert_eq!(value.as_object().unwrap().len(), 5);
            assert_eq!(transport.calls.borrow().len(), 20);
            for bucket in crate::recovery::clone::native::input::BUCKETS {
                let bytes =
                    format!("ryframe-owner:v1:seed-scope:object-storage:{bucket}").into_bytes();
                assert_eq!(value[bucket]["keys"], json!(["seed-scope/.ryframe-owner"]));
                assert_eq!(
                    value[bucket]["owner"],
                    json!({"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))})
                );
            }
        } else {
            assert!(result.is_err(), "{object_case}");
            assert_eq!(
                transport.calls.borrow().len(),
                if object_case == "nonempty" { 3 } else { 4 }
            );
        }
        for entry in std::fs::read_dir(&output).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|value| value == "json") {
                assert_public_object_receipt(&document::read(&path).unwrap(), false);
            }
        }
        assert!(!output.join("initialized.json").exists());
    }
}

fn assert_public_object_receipt(receipt: &Value, denied: bool) {
    document::exact(
        receipt,
        &[
            "transport",
            "operation",
            "bucket",
            "returncode",
            "error_type",
            "response",
            "key",
            "destination",
            "remote_writes",
            "scope_id",
            "endpoint",
            "configuration_sha256",
            "prefix",
            "continuation",
        ],
        "对象公开收据",
    )
    .unwrap();
    let public = serde_json::to_string(receipt).unwrap();
    assert!(!public.contains("fixture-access") && !public.contains("fixture-secret"));
    assert_eq!(receipt["transport"], "rust-s3-sigv4");
    assert_eq!(receipt["remote_writes"], 0);
    if denied {
        assert_eq!(receipt["error_type"], "NativeS3Error");
        assert!(receipt["returncode"].is_null());
        assert!(
            receipt["response"].is_null(),
            "原始失败正文不得进入response"
        );
    } else {
        assert!(receipt["error_type"].is_null());
        assert_eq!(receipt["returncode"], 0);
        assert!(receipt["response"].is_object());
    }
}

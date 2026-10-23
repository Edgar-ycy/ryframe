use super::{artifacts, execute, receipts};
use crate::recovery::clone::native::{digest, json as document, resources::common};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const SCOPE: &str = "department-fixture";

fn target() -> Value {
    json!({"scope_id":SCOPE,"endpoint":"http://127.0.0.1:19000","configuration_sha256":"a".repeat(64)})
}
fn receipt(operation: &str) -> Value {
    json!({"transport":"rust-s3-sigv4","operation":operation,"bucket":"uploads",
        "returncode":0,"error_type":null,"response":{},"key":null,"destination":null,
        "remote_writes":0,"scope_id":SCOPE,"endpoint":target()["endpoint"],
        "configuration_sha256":target()["configuration_sha256"],"prefix":null,"continuation":null})
}
fn owner() -> Value {
    let mut value = receipt("get-object");
    value["key"] = json!(format!("{SCOPE}/.ryframe-owner"));
    value["destination"] = json!(format!("owner-target-uploads-{}.txt", "1".repeat(32)));
    value["response"] = json!({"ContentLength":format!("ryframe-owner:v1:{SCOPE}:object-storage:uploads").len(),
        "Metadata":{},"ETag":"etag","LastModified":"date","ContentType":"text/plain"});
    value
}
fn listing() -> Value {
    let mut value = receipt("list-objects-v2");
    value["prefix"] = json!(format!("{SCOPE}/"));
    value["response"] =
        json!({"IsTruncated":false,"Contents":[{"Key":format!("{SCOPE}/.ryframe-owner")} ]});
    value
}
fn command(argv: Value) -> Value {
    json!({"command":argv,"returncode":0,"error_type":null,"stdout":"","stderr":""})
}

#[test]
fn accepts_all_native_read_only_operations_without_command_fallback() {
    for value in [
        receipt("head-bucket"),
        receipt("get-bucket-versioning"),
        owner(),
        listing(),
    ] {
        receipts::read_only(&value, &target()).unwrap();
    }
    let mut version = receipt("get-bucket-versioning");
    version["response"] = json!({"MFADelete":"Disabled"});
    receipts::read_only(&version, &target()).unwrap();
    assert!(
        receipts::read_only(
            &command(json!(["aws.exe", "s3api", "get-object"])),
            &target()
        )
        .is_err()
    );
}

#[test]
fn rejects_missing_fields_wrong_types_and_disguised_remote_writes() {
    let valid = owner();
    for field in valid.as_object().unwrap().keys() {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(receipts::read_only(&value, &target()).is_err(), "{field}");
    }
    for (field, change) in [
        ("operation", json!("put-object")),
        ("operation", json!([])),
        ("remote_writes", json!(1)),
        ("remote_writes", json!(false)),
        ("returncode", json!(false)),
        ("returncode", json!(1)),
        ("error_type", json!("TimeoutError")),
        ("bucket", json!("unknown")),
        ("bucket", json!([])),
        ("transport", json!("aws-cli")),
        ("response", json!([])),
        ("unknown", json!("field")),
        ("configuration_sha256", json!("b".repeat(64))),
        ("endpoint", json!("http://other")),
        ("scope_id", json!("another-scope")),
        ("continuation", json!("next")),
    ] {
        let mut value = valid.clone();
        value[field] = change;
        assert!(receipts::read_only(&value, &target()).is_err(), "{field}");
    }
}

#[test]
fn rejects_owner_scope_path_metadata_and_incomplete_listing() {
    for change in [
        json!("other/.ryframe-owner"),
        json!("department-fixture/business.bin"),
        Value::Null,
    ] {
        let mut value = owner();
        value["key"] = change;
        assert!(receipts::read_only(&value, &target()).is_err());
    }
    for change in [
        "../owner.txt",
        "C:\\owner.txt",
        "owner-target-avatar-11111111111111111111111111111111.txt",
    ] {
        let mut value = owner();
        value["destination"] = json!(change);
        assert!(receipts::read_only(&value, &target()).is_err());
    }
    for response in [
        json!({}),
        json!({"ContentLength":false}),
        json!({"ContentLength":0,"Metadata":{}}),
    ] {
        let mut value = owner();
        value["response"] = response;
        assert!(receipts::read_only(&value, &target()).is_err());
    }
    let mut invalid_length = owner();
    invalid_length["response"]["ContentLength"] = json!(false);
    assert!(receipts::read_only(&invalid_length, &target()).is_err());
    let mut missing_destination = owner();
    missing_destination["destination"] = Value::Null;
    assert!(receipts::read_only(&missing_destination, &target()).is_err());
    let mut duplicate = listing();
    duplicate["response"]["Contents"] = json!([
        {"Key":format!("{SCOPE}/.ryframe-owner")}, {"Key":format!("{SCOPE}/.ryframe-owner")}
    ]);
    assert!(receipts::read_only(&duplicate, &target()).is_err());
    for scope in [
        Value::Null,
        json!(""),
        json!("../scope"),
        json!("bad\nscope"),
    ] {
        let mut expected = target();
        expected["scope_id"] = scope;
        assert!(receipts::read_only(&owner(), &expected).is_err());
    }
    for response in [
        json!({}),
        json!({"IsTruncated":true,"Contents":[]}),
        json!({"IsTruncated":0,"Contents":[]}),
        json!({"IsTruncated":false,"Contents":[]}),
        json!({"IsTruncated":false,"Contents":[{"Key":"other/.ryframe-owner"}]}),
        json!({"IsTruncated":false,"Contents":[{"Key":format!("{SCOPE}/.ryframe-owner")}],"NextContinuationToken":"next"}),
    ] {
        let mut value = listing();
        value["response"] = response;
        assert!(receipts::read_only(&value, &target()).is_err());
    }
    for operation in ["head-bucket", "get-bucket-versioning", "list-objects-v2"] {
        let mut value = if operation == "list-objects-v2" {
            listing()
        } else {
            receipt(operation)
        };
        value["destination"] = json!("unexpected.txt");
        assert!(receipts::read_only(&value, &target()).is_err());
    }
    let mut value = receipt("get-bucket-versioning");
    value["response"] = json!({"Status":"Enabled"});
    assert!(receipts::read_only(&value, &target()).is_err());
}

#[test]
fn restricts_external_commands_and_preserves_exit_failures() {
    for argv in [
        json!(["cargo", "-V"]),
        json!(["rustc", "-Vv"]),
        json!(["mysql.exe", "--execute", " SELECT 1"]),
        json!([
            "powershell.exe",
            "Get-CimInstance Win32_Process | Select-Object -ExpandProperty .CommandLine | ConvertTo-Json -Compress"
        ]),
        json!(["wsl.exe", "/usr/bin/readlink"]),
    ] {
        let value = command(argv);
        receipts::read_only(&value, &Value::Null).unwrap();
        for (field, invalid) in [
            ("returncode", json!(17)),
            ("error_type", json!("Cancelled")),
            ("returncode", json!(false)),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(receipts::read_only(&changed, &Value::Null).is_err());
        }
    }
    for argv in [
        json!([]),
        json!(["node", "script.js"]),
        json!(["cargo", "build"]),
        json!(["mysql.exe", "--execute", "UPDATE t SET a=1"]),
        json!(["powershell.exe", "Remove-Item x"]),
        json!(["wsl.exe", "rm"]),
    ] {
        assert!(receipts::read_only(&command(argv), &Value::Null).is_err());
    }
}

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
    output: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend
            .join(".local-tests")
            .join(format!("department-中文 空格-{}", uuid::Uuid::new_v4()));
        let output = run.join("seed-runtime/attempt-0001");
        fs::create_dir_all(&output).unwrap();
        Self {
            backend,
            run,
            output,
        }
    }
    fn write(&self, name: &str, value: &Value) {
        document::write_new(&self.output.join(name), value).unwrap();
    }
    fn command(&self) {
        self.write(
            &format!("command-{}.json", "a".repeat(32)),
            &command(json!(["cargo", "-V"])),
        );
    }
    fn inspect(&self) -> crate::Result<Value> {
        artifacts::inspect(&self.backend, &self.output, &target())
    }
    fn args(&self, operation: &str) -> Value {
        json!({"operation":operation,"output":self.output,"target":target(),"attempt":1})
    }
    fn invoke(&self, operation: &str) -> crate::Result<Value> {
        execute(&self.backend, &self.run, &self.args(operation))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn owner_payload_requires_exact_body_unique_receipt_and_closed_file_set() {
    let fixture = Fixture::new();
    let value = owner();
    let body = fixture.output.join(value["destination"].as_str().unwrap());
    fs::write(
        &body,
        format!("ryframe-owner:v1:{SCOPE}:object-storage:uploads"),
    )
    .unwrap();
    assert!(fixture.inspect().is_err());
    fixture.write(&format!("command-{}.json", "1".repeat(32)), &value);
    assert_eq!(fixture.inspect().unwrap().as_array().unwrap().len(), 2);
    fs::write(&body, "wrong-owner").unwrap();
    assert!(fixture.inspect().is_err());
    fs::write(
        &body,
        format!("ryframe-owner:v1:{SCOPE}:object-storage:uploads"),
    )
    .unwrap();
    fixture.write(&format!("command-{}.json", "2".repeat(32)), &value);
    assert!(fixture.inspect().is_err());
    fs::remove_file(&body).unwrap();
    assert!(fixture.inspect().is_err());
}

#[test]
fn guard_manifest_cannot_be_overwritten_or_survive_drift_unknown_files_and_incomplete_write() {
    let fixture = Fixture::new();
    fixture.command();
    assert_eq!(fixture.invoke("inspect").unwrap(), Value::Null);
    let saved = fixture.invoke("publish").unwrap();
    assert_eq!(fixture.invoke("inspect").unwrap(), saved);
    assert!(fixture.invoke("publish").is_err());
    fixture.write("session-process.json", &json!({"unknown_write":true}));
    assert!(fixture.invoke("inspect").is_err());
    fs::remove_file(fixture.output.join("session-process.json")).unwrap();
    let command = fixture
        .output
        .join(format!("command-{}.json", "a".repeat(32)));
    fs::write(&command, b"{\"cancelled\":").unwrap();
    assert!(fixture.invoke("inspect").is_err());
    assert_eq!(
        common::binding(&fixture.output.join("adoption-guard.json")).unwrap(),
        saved
    );
}

#[test]
fn mysql_payload_binding_retry_and_unknown_outputs_are_closed() {
    let fixture = Fixture::new();
    let name = format!("mysql-identity-{}", "1".repeat(32));
    let stdout = fixture.output.join(format!("{name}.stdout"));
    fs::write(&stdout, "server-uuid").unwrap();
    let value = json!({"format_version":1,"kind":"mysql-verification-output","check":"identity","target_key":"control",
        "returncode":0,"error_type":null,"stderr_bytes":0,"stderr_sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "stdout_file":common::binding(&stdout).unwrap()});
    fixture.write(&format!("{name}.json"), &value);
    fixture.inspect().unwrap();
    fixture.write(&format!("mysql-identity-retry-{}.json", "2".repeat(32)), &json!({
        "format_version":1,"kind":"mysql-verification-retry","check":"identity","target_key":"control",
        "reason":"first_returncode_zero_stdout_stderr_exactly_empty","first":common::binding(&fixture.output.join(format!("{name}.json"))).unwrap(),
        "second":common::binding(&fixture.output.join(format!("{name}.json"))).unwrap()}));
    fixture.inspect().unwrap();
    fs::write(&stdout, "drift").unwrap();
    assert!(fixture.inspect().is_err());
}

#[test]
fn protocol_rejects_outside_attempt_and_wrong_sequence_without_creating_manifest() {
    let fixture = Fixture::new();
    fixture.command();
    let mut args = fixture.args("publish");
    args["attempt"] = json!(2);
    assert!(execute(&fixture.backend, &fixture.run, &args).is_err());
    args["output"] = json!(fixture.run);
    assert!(execute(&fixture.backend, &fixture.run, &args).is_err());
    assert!(!fixture.output.join("adoption-guard.json").exists());
    assert!(
        digest::file(
            &fixture
                .output
                .join(format!("command-{}.json", "a".repeat(32)))
        )
        .is_ok()
    );
}

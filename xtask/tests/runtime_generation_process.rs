use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn invoke(request: &Value) -> (bool, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__source-evidence")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-do-not-echo"));
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}
fn fixture() -> (tempfile::TempDir, Value) {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/runtime-generation-process");
    fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let coordinator = temporary.path();
    let local = coordinator.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let root = local.join("runtime");
    let descriptor =
        |name: &str| json!({"path":local.join(name),"bytes":2,"sha256":"a".repeat(64)});
    let binding = json!({"registration":descriptor("registration.json"),"target_plan":descriptor("target-plan.json")});
    let request = json!({"operation":"runtime-generation","write":true,"arguments":{
        "action":"create-intent","root":root,"binding":binding,"previous":null,"commands":null,"number":1,
        "value":{"registration":binding,"runtime_directory":root,
            "tools":{"coordinator_root":coordinator},"authority":{"scope_id":"test-runtime"}}
    }});
    (temporary, request)
}
#[test]
fn intent_requires_explicit_write_and_retains_unknown_or_conflicting_output() {
    let (_temporary, mut request) = fixture();
    let root = std::path::PathBuf::from(request["arguments"]["root"].as_str().unwrap());
    let intent = root.with_file_name("runtime.runtime-create.json");
    request["write"] = json!(false);
    assert!(!invoke(&request).0);
    assert!(!intent.exists());
    request["write"] = json!(true);
    let (ok, result) = invoke(&request);
    assert!(ok, "{result}");
    let original = fs::read(&intent).unwrap();
    assert!(!invoke(&request).0);
    assert_eq!(fs::read(&intent).unwrap(), original);
    fs::write(&intent, b"{\"partial\":").unwrap();
    let (ok, result) = invoke(&request);
    assert!(!ok);
    assert_eq!(result["error"]["automatic_retry_permitted"], false);
    assert_eq!(fs::read(&intent).unwrap(), b"{\"partial\":");
    assert!(!root.exists());
}
#[test]
fn generation_rejects_invalid_commands_extra_fields_and_boolean_numbers_without_writes() {
    let (_temporary, mut request) = fixture();
    request["write"] = json!(false);
    request["arguments"]["action"] = json!("new-generation");
    request["arguments"]["number"] = json!(true);
    let executable = Path::new(env!("CARGO_BIN_EXE_xtask"));
    request["arguments"]["commands"] =
        json!({"api":[executable],"worker":[executable],"frontend":[executable]});
    assert!(!invoke(&request).0);
    request["arguments"]["number"] = json!(1);
    let (ok, result) = invoke(&request);
    assert!(ok, "{result}");
    assert_eq!(result["result"]["value"]["status"], "starting");
    request["arguments"]["commands"]["frontend"] = json!(["relative-program"]);
    assert!(!invoke(&request).0);
    request["arguments"]["unexpected"] = json!("private-do-not-echo");
    assert!(!invoke(&request).0);
    let root = Path::new(request["arguments"]["root"].as_str().unwrap());
    assert!(!root.exists());
}

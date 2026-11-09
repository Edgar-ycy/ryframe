use super::*;
use std::fs;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    binding: Value,
    request: Value,
    commands: Value,
}
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/runtime-generation-tests");
        fs::create_dir_all(&base).unwrap();
        let temporary = tempfile::tempdir_in(base).unwrap();
        let coordinator = temporary.path().to_path_buf();
        let local = coordinator.join(".local-tests");
        fs::create_dir(&local).unwrap();
        let root = local.join("runtime");
        let descriptor =
            |name: &str| json!({"path":local.join(name),"bytes":2,"sha256":"1".repeat(64)});
        let binding = json!({"registration":descriptor("registration.json"),"target_plan":descriptor("target.json")});
        let request = json!({"registration":binding,"runtime_directory":root,
            "tools":{"coordinator_root":coordinator},"authority":{"scope_id":"runtime-test"}});
        let exe = std::env::current_exe().unwrap();
        let commands = json!({"api":[exe],"worker":[exe],"frontend":[exe]});
        Self {
            _temporary: temporary,
            root,
            binding,
            request,
            commands,
        }
    }
    fn generation(&self) -> Value {
        new_generation(&self.root, 1, &self.request, &self.commands).unwrap()
    }
    fn state(&self) -> Value {
        let creation = create_intent(&self.root, &self.binding, &self.request).unwrap();
        fs::create_dir(&self.root).unwrap();
        json!({"format_version":1,"kind":"restore-runtime-lifecycle","runtime_directory":self.root,
            "registration":self.binding,"creation_intent":creation,"generations":[self.generation()]})
    }
    fn seed_processes(&self, generation: &mut Value) {
        let directory = self.root.join("generation-0001");
        fs::create_dir(&directory).unwrap();
        for role in ROLES {
            let identity =
                json!({"pid":20,"started":"100","executable":std::env::current_exe().unwrap()});
            let supervisor =
                json!({"pid":21,"started":"101","executable":std::env::current_exe().unwrap()});
            let monitor =
                json!({"pid":22,"started":"102","executable":std::env::current_exe().unwrap()});
            let item = &mut generation["roles"][role];
            let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":directory,
                "role":role,"scope_id":"runtime-test","operation_id":item["operation_id"],
                "process":identity,"supervisor":supervisor,"monitor":monitor,"group_id":21});
            strict::write_new(&directory.join(format!("{role}-tree.json")), &tree).unwrap();
            strict::write_new(&directory.join(format!("{role}.json")),&json!({"format_version":1,"role":role,"scope_id":"runtime-test","identity":identity})).unwrap();
            fs::write(directory.join(format!("{role}.log")), b"").unwrap();
            item["tree"] = tree;
            item["state"] = json!("running");
        }
    }
}

#[test]
fn creation_is_exclusive_bound_and_preserved_on_unknown_result() {
    let f = Fixture::new();
    let receipt = create_intent(&f.root, &f.binding, &f.request).unwrap();
    assert!(create_intent(&f.root, &f.binding, &f.request).is_err());
    files::bound(&creation_path(&f.root), &receipt).unwrap();
    let mut value = files::document(&creation_path(&f.root)).unwrap();
    for replacement in [json!(true), json!(2)] {
        value["generation"] = replacement;
        assert!(validate_creation(&value, &f.root, &f.binding).is_err());
    }
    assert!(!f.root.exists());
}
#[test]
fn state_preserves_first_directory_cut_and_rejects_unknown_files_and_stale_preimage() {
    let f = Fixture::new();
    let state = f.state();
    let commands = json!([f.commands]);
    let previous = save_state(&f.root, &state, &f.binding, &Value::Null, &commands).unwrap();
    validate_state(&state, &f.root, &f.binding, &commands).unwrap();
    assert!(save_state(&f.root, &state, &f.binding, &Value::Null, &commands).is_err());
    fs::write(f.root.join("foreign.json"), b"{}").unwrap();
    assert!(validate_state(&state, &f.root, &f.binding, &commands).is_err());
    fs::remove_file(f.root.join("foreign.json")).unwrap();
    fs::write(f.root.join(STATE), b"{}").unwrap();
    assert!(save_state(&f.root, &state, &f.binding, &previous, &commands).is_err());
    assert_eq!(fs::read(f.root.join(STATE)).unwrap(), b"{}");
}
#[test]
fn roles_reject_duplicate_operations_boolean_numbers_and_command_drift() {
    let f = Fixture::new();
    let original = f.generation();
    let mut value = original.clone();
    value["roles"]["worker"]["operation_id"] = value["roles"]["api"]["operation_id"].clone();
    assert!(validate_generation(&value, &f.root, 1, &f.binding, &f.commands).is_err());
    for field in ["number", "status", "directory"] {
        let mut value = original.clone();
        value[field] = json!(true);
        assert!(validate_generation(&value, &f.root, 1, &f.binding, &f.commands).is_err());
    }
    value = original;
    value["request"]["runtime_directory"] = json!(f.root.join("other"));
    assert!(validate_generation(&value, &f.root, 1, &f.binding, &f.commands).is_err());
    value["request"]["runtime_directory"] = json!(f.root);
    value["roles"]["api"]["command"] = json!(["unexpected"]);
    assert!(validate_generation(&value, &f.root, 1, &f.binding, &f.commands).is_err());
}
#[test]
fn launch_publication_cut_accepts_only_complete_matching_trees() {
    let f = Fixture::new();
    let mut state = f.state();
    let commands = json!([f.commands]);
    save_state(&f.root, &state, &f.binding, &Value::Null, &commands).unwrap();
    let generation = &mut state["generations"][0];
    f.seed_processes(generation);
    let directory = f.root.join("generation-0001");
    let binding = receipts::write_launch(&directory, generation, &f.commands).unwrap();
    assert!(receipts::write_launch(&directory, generation, &f.commands).is_err());
    validate_generation(generation, &f.root, 1, &f.binding, &f.commands).unwrap();
    generation["status"] = json!("running");
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
    generation["launch"] = binding;
    validate_generation(generation, &f.root, 1, &f.binding, &f.commands).unwrap();
    let path = directory.join("api.json");
    let mut changed = files::document(&path).unwrap();
    changed["identity"]["started"] = json!("101");
    strict::write_replace(&path, &changed).unwrap();
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
}
#[test]
fn completion_is_bound_to_exact_role_operation_and_bytes() {
    let f = Fixture::new();
    let mut state = f.state();
    f.seed_processes(&mut state["generations"][0]);
    let generation = &mut state["generations"][0];
    let directory = f.root.join("generation-0001");
    let operation = text(&generation["roles"]["api"]["operation_id"]).unwrap();
    let path = directory.join(format!("api-members-{operation}-stopped.json"));
    strict::write_new(&path, &json!({"status":"stopped"})).unwrap();
    generation["roles"]["api"]["completion"] = files::bound_value(&path).unwrap();
    validate_generation(generation, &f.root, 1, &f.binding, &f.commands).unwrap();
    fs::write(path, b"{}").unwrap();
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
}
#[test]
fn writes_cannot_escape_bound_coordinator_local_directory() {
    let f = Fixture::new();
    let root = f.root.parent().unwrap().parent().unwrap().join("foreign");
    assert!(create_intent(&root, &f.binding, &f.request).is_err());
    assert!(!creation_path(&root).exists());
    assert!(files::safe(&f.root.join("../foreign")).is_err());
}
#[test]
fn process_directory_rejects_unknown_files_and_operation_mismatch() {
    let f = Fixture::new();
    let mut state = f.state();
    f.seed_processes(&mut state["generations"][0]);
    let generation = &mut state["generations"][0];
    let directory = f.root.join("generation-0001");
    fs::write(directory.join("foreign"), b"{}").unwrap();
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
    fs::remove_file(directory.join("foreign")).unwrap();
    generation["roles"]["api"]["operation_id"] = json!("0".repeat(32));
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
}
#[test]
fn strict_document_rejects_duplicate_keys_and_truncated_json_without_echo() {
    let f = Fixture::new();
    let path = f.root.parent().unwrap().join("bad.json");
    for data in [
        b"{\"token\":\"secret\",\"token\":2}".as_slice(),
        b"{\"secret\":".as_slice(),
    ] {
        fs::write(&path, data).unwrap();
        let error = files::document(&path).unwrap_err().to_string();
        assert!(!error.contains("secret"));
    }
}

#[test]
fn document_rejects_empty_and_oversized_inputs_before_unbounded_read() {
    let f = Fixture::new();
    let path = f.root.parent().unwrap().join("large.json");
    fs::write(&path, b"").unwrap();
    assert!(files::document(&path).is_err());
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(strict::MAX_JSON_BYTES + 1)
        .unwrap();
    assert!(files::document(&path).is_err());
}

#[test]
fn interrupted_stop_retains_exact_monitor_proof_without_inventing_product_receipt() {
    let f = Fixture::new();
    let mut state = f.state();
    let generation = &mut state["generations"][0];
    f.seed_processes(generation);
    generation["status"] = json!("stopped");
    let directory = f.root.join("generation-0001");
    let product = directory.join("api.json");
    fs::remove_file(&product).unwrap();
    generation["roles"]["api"]["state"] = json!("stopped");
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());

    let tree = &generation["roles"]["api"]["tree"];
    let operation = text(&tree["operation_id"]).unwrap();
    let path = directory.join(format!("api-members-{operation}-stopped.json"));
    let mut proof = json!({
        "format_version":1, "operation_id":operation, "directory":directory,
        "role":"api", "scope_id":"runtime-test", "supervisor":tree["supervisor"],
        "monitor":tree["monitor"], "status":"stopped",
        "members":[tree["supervisor"],tree["process"]], "error_type":null,
    });
    strict::write_new(&path, &proof).unwrap();
    generation["roles"]["api"]["completion"] = files::bound_value(&path).unwrap();
    validate_generation(generation, &f.root, 1, &f.binding, &f.commands).unwrap();
    assert!(!product.exists());

    proof["scope_id"] = json!("unknown");
    strict::write_replace(&path, &proof).unwrap();
    generation["roles"]["api"]["completion"] = files::bound_value(&path).unwrap();
    assert!(validate_generation(generation, &f.root, 1, &f.binding, &f.commands).is_err());
    assert!(!product.exists());
}

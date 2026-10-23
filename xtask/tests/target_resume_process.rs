//! 续作证据及状态转换的真实进程测试；不调用 Python 或共享服务。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Fixture {
    backend: PathBuf,
    directory: tempfile::TempDir,
    context: Value,
}

impl Fixture {
    fn transition(&self, transition: &str, arguments: Value) -> Value {
        self.invoke(
            "resume-transition",
            json!({"transition":transition,"arguments":arguments}),
        )
    }

    fn complete(&self, session: &Value, operation: &Value) -> Value {
        let attempt = "a".repeat(32);
        let index = session["next_index"].as_u64().unwrap();
        let intent = self.write(&format!("resume-migrate-{attempt}-{index:02}.intent.json"), &json!({
            "format_version":1,"kind":"devex-clone-migration-operation-intent","attempt":attempt,"index":index,"at":"2026-10-23T00:00:01Z","resume":{},"failure":{},
            "operation":{"id":operation["id"],"command":operation["command"],"write":operation["write"]}}));
        let receipt = self.write(&format!("{}-{}.command.json",operation["stage"].as_str().unwrap(),"a".repeat(32)),
            &json!({"command":operation["command"],"returncode":0,"error_type":null,"stdout":operation["stdout"],"stderr":""}));
        let confirmed = self.transition("operation-confirmed", json!({"attempt":attempt,"index":index,"at":"2026-10-23T00:00:02Z","intent":intent,"receipt":receipt,"write":operation["write"]}));
        assert_eq!(confirmed["ok"], true, "{confirmed}");
        let confirmation = self.write(
            &format!("resume-migrate-{attempt}-{index:02}.confirmed.json"),
            &confirmed["result"],
        );
        self.transition(
            "session-completed",
            json!({"session":session,"operation":operation,"completed":{
            "id":operation["id"],"receipt":receipt,"confirmation":confirmation}}),
        )
    }

    fn new() -> Self {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let parent = backend.join(".local-tests/target-resume-native-process");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("中文 空格-")
            .tempdir_in(parent)
            .unwrap();
        let context = json!({"failure":{"path":"source","bytes":0,"sha256":"a".repeat(64)},
            "source_failure_at":"2026-10-23T00:00:00Z","prepared_files":{},
            "plan":{"manifest":{},"plan_hash":hex::encode(Sha256::digest(b"{}"))},
            "executable":"fixed-reset.exe","request":{"target":{"scope_id":"fixture","databases":[]}},"available":[]});
        Self {
            backend,
            directory,
            context,
        }
    }

    fn write(&self, name: &str, value: &Value) -> Value {
        let path = self.directory.path().join(name);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        binding(&path)
    }

    fn records(&self, success: Value) -> Value {
        let mut context = self.context.clone();
        context["available"] = json!(tree(self.directory.path()).keys().collect::<Vec<_>>());
        self.invoke(
            "inventory-resume-records",
            json!({"context":context,"success":success}),
        )
    }

    fn invoke(&self, operation: &str, fields: Value) -> Value {
        let mut request = json!({"format_version":1,"operation":operation,"backend":self.backend,"target":self.directory.path()});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let before = tree(self.directory.path());
        let output = invoke(
            &["__target-evidence"],
            &serde_json::to_vec(&request).unwrap(),
        );
        assert!(output.stderr.is_empty());
        assert_eq!(tree(self.directory.path()), before, "证据核验不能改写现场");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.success(), value["ok"] == true);
        if value["ok"] == false {
            assert_eq!(value["error"]["remote_writes"], 0);
            assert_eq!(value["error"]["automatic_retry"], false);
        }
        value
    }

    fn retry(&self, id: u8, from: &str, to: &str) {
        let attempt = format!("{id:032x}");
        let intent = self.write(&format!("resume-initialize-{attempt}.intent.json"), &json!({"format_version":1,"kind":"devex-clone-initialize-resume-intent", "attempt":attempt,"at":from,
            "failure":self.context["failure"],"prepared_files":self.context["prepared_files"],"remote_writes":0}));
        self.write(&format!("resume-initialize-{attempt}.failure.json"), &json!({"format_version":1,"kind":"devex-clone-initialize-resume-failure", "attempt":attempt,"at":to,
            "intent":intent,"source_failure":self.context["failure"],"prepared_files":self.context["prepared_files"],"error_type":"InventoryCaptureError","remote_writes":0}));
        self.write(
            &format!("resume-reset-plan-{attempt}.json"),
            &self.context["plan"],
        );
        self.write(&format!("resume-reset-plan-{attempt}-{}.command.json", "a".repeat(32)), &json!({"command":["fixed-reset.exe","plan"],"returncode":0,"error_type":null,
            "stdout":format!("{{}}\nplan_hash={}\n", self.context["plan"]["plan_hash"].as_str().unwrap()),"stderr":""}));
        let directory = self
            .directory
            .path()
            .join(format!("inventory-resume-{attempt}"));
        fs::create_dir(&directory).unwrap();
        self.write(&format!("inventory-resume-{attempt}/failure.json"), &json!({"format_version":1,"status":"side_inventory_failed","stage":"inputs","error_type":"FileNotFoundError","remote_writes":0,"target_ready":false,"clone_verified":false}));
    }
}

fn invoke(args: &[&str], bytes: &[u8]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .args(args)
        .env("PATH", "")
        .env_remove("APP_ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(bytes);
    child.wait_with_output().unwrap()
}

fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({"path":path,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))})
}

fn tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut entries = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for path in fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
        {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            if path.is_dir() {
                entries.insert(name, Vec::new());
                pending.push(path);
            } else {
                entries.insert(name, fs::read(path).unwrap());
            }
        }
    }
    entries
}

#[test]
fn inventory_records_accept_empty_and_complete_readonly_retries() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.records(Value::Null)["result"],
        json!({"attempts":0,"files":[]})
    );
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:02Z");
    fixture.retry(2, "2026-10-23T00:00:02+00:00", "2026-10-23T00:00:03Z");
    let value = fixture.records(Value::Null);
    assert_eq!(value["result"]["attempts"], 2, "{value}");
    assert_eq!(value["result"]["files"].as_array().unwrap().len(), 12);
}

#[test]
fn inventory_records_reject_pending_unknown_writes_and_time_overlap() {
    let fixture = Fixture::new();
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:04Z");
    fixture.retry(2, "2026-10-23T00:00:03Z", "2026-10-23T00:00:05Z");
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存续作失败尝试时间谱系重叠"
    );
    let failure = format!("resume-initialize-{:032x}.failure.json", 2);
    fs::remove_file(fixture.directory.path().join(&failure)).unwrap();
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存续作包含挂起、重复或未知结果"
    );
    fixture.write(&failure, &json!({"format_version":1,"kind":"devex-clone-initialize-resume-failure","attempt":format!("{:032x}",2),"at":"2026-10-23T00:00:05Z","intent":{},"source_failure":fixture.context["failure"],"prepared_files":{},"error_type":"TimeoutExpired","remote_writes":1}));
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存续作失败收据不是只读失败"
    );
}

#[test]
fn inventory_failure_closed_set_rejects_unknown_and_unreferenced_stdout() {
    let fixture = Fixture::new();
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:02Z");
    let directory = format!("inventory-resume-{:032x}", 1);
    fixture.write(
        &format!("{directory}/unknown.json"),
        &json!({"password":"never-echo-fixture-secret"}),
    );
    let response = fixture.records(Value::Null);
    assert_eq!(response["error"]["message"], "库存失败目录包含未知证据");
    assert!(!response.to_string().contains("never-echo-fixture-secret"));
    fs::remove_file(
        fixture
            .directory
            .path()
            .join(&directory)
            .join("unknown.json"),
    )
    .unwrap();
    fs::write(
        fixture
            .directory
            .path()
            .join(&directory)
            .join("orphan.stdout"),
        b"private",
    )
    .unwrap();
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败 stdout 文件集合与命令收据引用不匹配"
    );
}

#[test]
fn malformed_confirmations_and_request_extensions_fail_closed() {
    let fixture = Fixture::new();
    let response = fixture.invoke("inventory-resume-confirmed", json!({"initialized":{}}));
    assert_eq!(response["result"], json!({"confirmed":false}));
    for id in [1, 2] {
        fixture.write(
            &format!("resume-initialize-{id:032x}.confirmed.json"),
            &json!({"password":"never-echo-fixture-secret"}),
        );
        assert_eq!(
            fixture.invoke("inventory-resume-confirmed", json!({"initialized":{}}))["result"]["confirmed"],
            false
        );
    }
    for extra in [
        json!({"initialized":{},"extra":true}),
        json!({"initialized":{},"format_version":false}),
    ] {
        let response = fixture.invoke("inventory-resume-confirmed", extra);
        assert_eq!(response["ok"], false);
        assert!(!response.to_string().contains("never-echo-fixture-secret"));
    }
}

#[test]
fn failure_transition_retains_unknown_write_and_strips_private_receipt_path() {
    let fixture = Fixture::new();
    let attempt = "a".repeat(32);
    let intent = fixture.write(
        &format!("resume-initialize-{attempt}.intent.json"),
        &json!({}),
    );
    let mut arguments = json!({"attempt":attempt,"at":"2026-10-23T00:00:02Z","intent":intent,"source_failure":{},"prepared_files":{},"started":null,"error_type":"TimeoutExpired","confirmed_operations":[],"remote_write_operations_confirmed":1,"active_write":null,
        "active_operation":{"write_started":true,"unknown_result":true,"phase":"command-running","_receipt_path":"never-echo-fixture-secret"},"os_error":null,"predecessor":null});
    let response = fixture.invoke(
        "resume-transition",
        json!({"transition":"migration-failure","arguments":arguments}),
    );
    assert_eq!(
        response["result"]["unknown_write_operation"],
        "redis-marker-rebind"
    );
    assert_eq!(response["result"]["remote_write_operations_confirmed"], 1);
    assert!(!response.to_string().contains("never-echo-fixture-secret"));
    assert!(response["result"].get("predecessor").is_none());
    arguments["active_write"] = json!("tenant-data-dedicated-b-up");
    arguments["active_operation"]["write_started"] = json!(false);
    let response = fixture.invoke(
        "resume-transition",
        json!({"transition":"migration-failure","arguments":arguments}),
    );
    assert_eq!(
        response["result"]["unknown_write_operation"],
        "tenant-data-dedicated-b-up"
    );
}

#[test]
fn private_protocol_refuses_argv_duplicate_fields_and_oversized_input() {
    for (args, bytes) in [
        (vec!["__target-evidence", "unexpected"], b"{}".to_vec()),
        (
            vec!["__target-evidence"],
            br#"{"operation":"inventory-resume-confirmed","operation":"resume-transition"}"#
                .to_vec(),
        ),
        (vec!["__target-evidence"], vec![b'x'; 1024 * 1024 + 1]),
    ] {
        let output = invoke(&args, &bytes);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["error"]["remote_writes"], 0);
        assert_eq!(response["error"]["automatic_retry"], false);
    }
}

#[test]
fn os_error_diagnostic_normalizes_extended_drive_unc_and_unicode_paths() {
    let fixture = Fixture::new();
    for (filename, expected) in [
        (
            r"\\?\D:\中文 空格\receipt.json",
            r"D:\中文 空格\receipt.json",
        ),
        (
            r"\\?\UNC\server\share\中文\receipt.json",
            r"\\server\share\中文\receipt.json",
        ),
        (r"D:\中文🙂\receipt.json", r"D:\中文🙂\receipt.json"),
    ] {
        let response = fixture.invoke(
            "resume-transition",
            json!({"transition":"os-error","arguments":{
            "errno":2,"winerror":3,"filename":filename,"receipt_path":expected}}),
        );
        assert_eq!(
            response["result"],
            json!({"errno":2,"winerror":3,"filename_role":"command_receipt","filename_length":expected.chars().count()})
        );
        assert!(!response.to_string().contains("receipt.json"));
    }
    let response = fixture.invoke(
        "resume-transition",
        json!({"transition":"os-error","arguments":{
        "errno":13,"winerror":null,"filename":null,"receipt_path":null}}),
    );
    assert_eq!(
        response["result"],
        json!({"errno":13,"winerror":null,"filename_role":null,"filename_length":null})
    );
}

#[test]
fn inventory_stdout_binding_capture_failure_duplicates_and_escape_fail_closed() {
    let fixture = Fixture::new();
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:02Z");
    let directory = format!("inventory-resume-{:032x}", 1);
    let stdout = fixture
        .directory
        .path()
        .join(&directory)
        .join("capture.stdout");
    fs::write(&stdout, b"private-output").unwrap();
    let command_name = format!("{directory}/command-{}.json", "a".repeat(32));
    let command = json!({"command":["migrate","inventory"],"returncode":0,"error_type":null,
        "stdin":null,"stdout":null,"stderr":"","stdout_file":binding(&stdout)});
    fixture.write(&command_name, &command);
    assert_eq!(fixture.records(Value::Null)["ok"], true);
    fs::write(&stdout, b"changed-output").unwrap();
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败 stdout 文件绑定变化"
    );
    let mut capture = command.clone();
    capture["stdout_file"] = json!({"path":stdout});
    capture["stdout_capture_error"] = json!("FileNotFoundError");
    fixture.write(&command_name, &capture);
    assert_eq!(fixture.records(Value::Null)["ok"], true);
    let duplicate = format!("{directory}/command-{}.json", "b".repeat(32));
    fixture.write(&duplicate, &capture);
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败 stdout 文件被多个命令收据重复引用"
    );
    fs::remove_file(fixture.directory.path().join(&duplicate)).unwrap();
    capture["stdout_file"] = json!({"path":fixture.directory.path().join("outside.stdout")});
    fixture.write(&command_name, &capture);
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败 stdout 文件越出采集目录或名称无效"
    );
}

#[test]
fn inventory_raw_outputs_require_fixed_database_prefix_and_before_after_boundary() {
    let mut fixture = Fixture::new();
    let keys = ["shared-control", "shared", "dedicated-a", "dedicated-b"];
    fixture.context["request"]["target"]["databases"] = json!(keys.map(|key| json!({"key":key,"kind":"tenant","server_uuid":"server","database":key,"mode":"shared"})));
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:02Z");
    let directory = format!("inventory-resume-{:032x}", 1);
    let raw = |key: &str| {
        json!({"scope_id":"fixture","control_schema_fingerprint":"control","tenant_schema_fingerprint":"tenant","target":{
        "database":{"key":key,"kind":"tenant","server_uuid":"server","database":key,"shared":true,"placements":[],"tables":[]},"preserved_tables":[]}})
    };
    let gap = format!("{directory}/before-target-shared.json");
    fixture.write(&gap, &raw("shared"));
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败原始四库输出不是连续固定前缀"
    );
    fs::remove_file(fixture.directory.path().join(gap)).unwrap();
    let after = format!("{directory}/after-target-shared-control.json");
    fixture.write(&after, &raw("shared-control"));
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存 after 输出不能越过未完成的 before"
    );
    for key in keys {
        fixture.write(&format!("{directory}/before-target-{key}.json"), &raw(key));
    }
    assert_eq!(fixture.records(Value::Null)["ok"], true);
    let mut changed = raw("shared-control");
    changed["scope_id"] = json!("other-tenant");
    fixture.write(&after, &changed);
    assert_eq!(
        fixture.records(Value::Null)["error"]["message"],
        "库存失败原始目标输出不属于固定四库"
    );
}

#[test]
fn inventory_retry_rejects_terminal_fork_and_success_before_prior_failure() {
    let fixture = Fixture::new();
    fixture.retry(1, "2026-10-23T00:00:01Z", "2026-10-23T00:00:04Z");
    fixture.retry(2, "2026-10-23T00:00:03Z", "2026-10-23T00:00:05Z");
    let attempt = format!("{:032x}", 2);
    fixture.write(
        &format!("resume-initialize-{attempt}.confirmed.json"),
        &json!({}),
    );
    let success = json!({"attempt":attempt,"at":"2026-10-23T00:00:05Z"});
    assert_eq!(
        fixture.records(success.clone())["error"]["message"],
        "库存续作包含挂起、重复或未知结果"
    );
    fs::remove_file(
        fixture
            .directory
            .path()
            .join(format!("resume-initialize-{attempt}.failure.json")),
    )
    .unwrap();
    assert_eq!(
        fixture.records(success)["error"]["message"],
        "库存续作成功尝试早于既有失败尝试"
    );
}

#[test]
fn operation_confirmation_requires_successful_bound_receipt_and_preserves_unknown_failure() {
    let fixture = Fixture::new();
    let attempt = "a".repeat(32);
    let intent = fixture.write(&format!("resume-migrate-{attempt}-03.intent.json"), &json!({
        "format_version":1,"kind":"devex-clone-migration-operation-intent","attempt":attempt,"index":3,"at":"2026-10-23T00:00:01Z","resume":{},"failure":{},"operation":{"id":"tenant-data-dedicated-a-verify","command":["migrate","tenant-data","verify","--target","dedicated-a"],"write":false}}));
    let path = "migrate-tenant-data---target-dedicated-a-verify-00000000000000000000000000000000.command.json";
    let receipt = fixture.write(path, &json!({"command":["migrate","tenant-data","verify","--target","dedicated-a"],"returncode":0,"error_type":null,"stdout":"scope=tenant-data target=dedicated-a migration_ledger=current schema=verified\n","stderr":""}));
    let mut arguments = json!({"attempt":attempt,"index":3,"at":"2026-10-23T00:00:02Z","intent":intent,"receipt":receipt,"write":false});
    assert_eq!(
        fixture.invoke(
            "resume-transition",
            json!({"transition":"operation-confirmed","arguments":arguments})
        )["ok"],
        true
    );
    arguments["write"] = json!(true);
    assert_eq!(
        fixture.invoke(
            "resume-transition",
            json!({"transition":"operation-confirmed","arguments":arguments})
        )["ok"],
        false
    );
    arguments["write"] = json!(false);
    let failed = fixture.write(path, &json!({"command":["migrate","tenant-data","verify","--target","dedicated-a"],"returncode":1,"error_type":"TimeoutExpired","stdout":"verified","stderr":""}));
    arguments["receipt"] = failed;
    assert_eq!(
        fixture.invoke(
            "resume-transition",
            json!({"transition":"operation-confirmed","arguments":arguments})
        )["ok"],
        false
    );
}

#[test]
fn migration_session_advances_only_after_confirmation_and_never_retries_active_write() {
    let fixture = Fixture::new();
    let initial = json!({"resumable":true,"mode":"migration","next_index":3});
    let mut session =
        fixture.transition("session-start", json!({"state":initial}))["result"].clone();
    assert_eq!(session["confirmed_writes"], 0);
    let operations = fixture.invoke(
        "migration-operations",
        json!({"executable":"fixture-migrate.exe"}),
    )["result"]["operations"]
        .clone();
    session = fixture.transition(
        "session-operation",
        json!({"session":session,"operation":operations[3],"index":3}),
    )["result"]
        .clone();
    assert_eq!(session["active_write"], Value::Null);
    assert_eq!(
        fixture.transition(
            "session-operation",
            json!({"session":session,"operation":operations[3],"index":3})
        )["ok"],
        false
    );
    let completed = fixture.complete(&session, &operations[3]);
    assert_eq!(completed["ok"], true, "{completed}");
    session = completed["result"].clone();
    assert_eq!(session["next_index"], 4);
    assert_eq!(session["confirmed_writes"], 0);
    session = fixture.transition(
        "session-operation",
        json!({"session":session,"operation":operations[4],"index":4}),
    )["result"]
        .clone();
    assert_eq!(session["active_write"], "tenant-data-dedicated-b-up");
    assert_eq!(
        fixture.transition(
            "session-operation",
            json!({"session":session,"operation":operations[5],"index":5})
        )["ok"],
        false
    );
    let completed = fixture.complete(&session, &operations[4]);
    assert_eq!(completed["ok"], true, "{completed}");
    assert_eq!(completed["result"]["confirmed_writes"], 1);
    assert_eq!(completed["result"]["next_index"], 5);
    session["confirmed_writes"] = json!(u64::MAX);
    assert_eq!(fixture.complete(&session, &operations[4])["ok"], false);
}

#[test]
fn migration_session_rejects_write_boundaries_and_repeated_redis_rebinding() {
    let fixture = Fixture::new();
    for next in [0, 2, 10] {
        assert_eq!(
            fixture.transition(
                "session-start",
                json!({"state":{"resumable":true,"mode":"migration","next_index":next}})
            )["ok"],
            false
        );
    }
    let state = json!({"resumable":true,"mode":"migration-successor","next_index":3});
    let successor = fixture.transition("session-start", json!({"state":state}))["result"].clone();
    assert_eq!(successor["confirmed_writes"], 1);
    assert_eq!(
        fixture.transition("session-markers", json!({"session":successor}))["ok"],
        false
    );
    let session = fixture.transition(
        "session-start",
        json!({"state":{"resumable":true,"mode":"migration","next_index":3}}),
    )["result"]
        .clone();
    let active =
        fixture.transition("session-markers", json!({"session":session}))["result"].clone();
    assert_eq!(
        fixture.transition("session-markers", json!({"session":active}))["ok"],
        false
    );
    let confirmation = fixture.write("redis-marker-confirmed.json", &json!({"kind":"devex-clone-redis-marker-rebind-confirmed","remote_write_operations":1,"unknown_result":false}));
    let complete = fixture.transition(
        "session-markers-completed",
        json!({"session":active,"confirmation":confirmation}),
    );
    assert_eq!(complete["result"]["confirmed_writes"], 1);
    assert_eq!(complete["result"]["active_operation"], Value::Null);
    assert_eq!(
        fixture.transition("session-markers", json!({"session":complete["result"]}))["ok"],
        false
    );
}

fn migration_receipts(fixture: &Fixture, count: usize) -> Value {
    let operations = fixture.invoke(
        "migration-operations",
        json!({"executable":"D:/中文 空格/migrate.exe"}),
    )["result"]["operations"]
        .clone();
    for operation in operations.as_array().unwrap().iter().take(count) {
        fixture.write(
            &format!(
                "{}-{:032x}.command.json",
                operation["stage"].as_str().unwrap(),
                0
            ),
            &json!({"command":operation["command"],"stdout":operation["stdout"],
                "returncode":0,"error_type":null,"stderr":""}),
        );
    }
    operations
}

#[test]
fn migration_prefix_process_rejects_duplicate_or_noncontiguous_receipts() {
    let fixture = Fixture::new();
    let operations = migration_receipts(&fixture, 3);
    let check = || {
        fixture.invoke(
            "migration-prefix",
            json!({"operations":operations,"complete":false}),
        )
    };
    assert_eq!(check()["result"]["next_index"], 3);
    let original = format!("migrate-control-up-{:032x}.command.json", 0);
    let duplicate = format!("migrate-control-up-{:032x}.command.json", 1);
    fs::copy(
        fixture.directory.path().join(&original),
        fixture.directory.path().join(&duplicate),
    )
    .unwrap();
    assert_eq!(check()["error"]["message"], "迁移前缀包含重复命令收据");
    fs::remove_file(fixture.directory.path().join(duplicate)).unwrap();
    let future = &operations[4];
    fixture.write(
        &format!(
            "{}-{:032x}.command.json",
            future["stage"].as_str().unwrap(),
            0
        ),
        &json!({"command":future["command"],"stdout":future["stdout"],
            "returncode":0,"error_type":null,"stderr":""}),
    );
    assert_eq!(
        check()["error"]["message"],
        "迁移收据越过首个缺失操作，不能证明连续前缀"
    );
}

#[test]
fn migration_prefix_process_refuses_write_boundary_and_tampered_bound_file() {
    let fixture = Fixture::new();
    let operations = migration_receipts(&fixture, 2);
    let check = || {
        fixture.invoke(
            "migration-prefix",
            json!({"operations":operations,"complete":false}),
        )
    };
    assert_eq!(
        check()["error"]["message"],
        "迁移前缀的首个缺失操作必须是只读 verify"
    );
    let operation = &operations[2];
    let name = format!(
        "{}-{:032x}.command.json",
        operation["stage"].as_str().unwrap(),
        0
    );
    let value = json!({"command":operation["command"],"stdout":operation["stdout"],
        "returncode":0,"error_type":null,"stderr":""});
    let descriptor = fixture.write(&name, &value);
    let verified = check();
    assert_eq!(verified["result"]["completed"][2]["receipt"], descriptor);
    for (field, replacement) in [
        ("stdout", json!("private-secret-output")),
        ("returncode", json!(1)),
        ("error_type", json!("TimeoutExpired")),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement;
        fixture.write(&name, &changed);
        let response = check();
        assert_eq!(
            response["error"]["message"],
            "迁移前缀命令、退出状态或成功输出无效"
        );
        assert!(!response.to_string().contains("private-secret-output"));
    }
}

struct ResetFixture {
    fixture: Fixture,
    manifest: Value,
    hash: String,
    ledger: Value,
    report: Value,
    previous: Value,
}

impl ResetFixture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let manifest = json!({"environment":"test","scope_id":"resume-fixture", "code_sha":"a".repeat(40),
            "config_sha":"b".repeat(64),"credential_version":"fixture-v1",
            "databases":[{"host":"127.0.0.1","port":3306,"database":"fixture"}],
            "redis":{"namespace":"fixture:"},"object_storage":{"prefixes":[{"bucket":"uploads","prefix":"fixture/"}]}});
        let hash = "c".repeat(64);
        let phases: serde_json::Map<String, Value> = [
            "preflight",
            "object_storage",
            "redis",
            "databases",
            "control_baseline",
            "tenant_baselines",
            "verification",
            "release",
        ]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                json!({"status":"complete","completed_at":"2026-10-23T00:00:02Z"}),
            )
        })
        .collect();
        let mut ledger = json!({"ledger_version":4,"plan_hash":hash,"phases":phases,
            "resources":{"database":{"status":"complete"}},"updated_at":"2026-10-23T00:00:02Z"});
        for key in [
            "environment",
            "scope_id",
            "code_sha",
            "config_sha",
            "credential_version",
        ] {
            ledger[key] = manifest[key].clone();
        }
        let mut previous = ledger.clone();
        previous["updated_at"] = json!("2026-10-23T00:00:01Z");
        previous["phases"]["release"] = json!({"status":"running","completed_at":null});
        let mut report = ledger.clone();
        report.as_object_mut().unwrap().remove("ledger_version");
        report["report_version"] = json!(2);
        report["status"] = json!("completed");
        report["failed_phase"] = Value::Null;
        report["completed_at"] = json!("2026-10-23T00:00:02Z");
        report["databases"] = json!(["127.0.0.1:3306/fixture"]);
        report["redis_namespace"] = json!("fixture:");
        report["object_prefixes"] = json!(["uploads:fixture/"]);
        fs::create_dir(fixture.directory.path().join("reset-state")).unwrap();
        let result = Self {
            fixture,
            manifest,
            hash,
            ledger,
            report,
            previous,
        };
        result.publish();
        result
    }

    fn name(&self, suffix: &str) -> String {
        let stem = format!("test-resume-fixture-{}", self.hash);
        if suffix == "previous" {
            format!("reset-state/.{stem}.ledger.json.previous")
        } else {
            format!("reset-state/{stem}.{suffix}.json")
        }
    }

    fn publish(&self) {
        self.fixture.write(&self.name("report"), &self.report);
        self.fixture.write(&self.name("ledger"), &self.ledger);
        self.fixture.write(&self.name("previous"), &self.previous);
    }

    fn check(&self) -> Value {
        self.fixture.invoke(
            "reset-completed",
            json!({"manifest":self.manifest,"sha":self.hash}),
        )
    }
}

#[test]
fn reset_process_requires_completed_release_and_exact_previous_ledger() {
    let mut fixture = ResetFixture::new();
    assert_eq!(fixture.check()["result"]["status"], "completed");
    let original_report = fixture.report.clone();
    fixture.report["phases"]["release"]["status"] = json!("running");
    fixture.publish();
    assert_eq!(
        fixture.check()["error"]["message"],
        "reset 有未完成阶段或锁释放证据缺失"
    );
    fixture.report = original_report;
    fixture.publish();
    let path = fixture
        .fixture
        .directory
        .path()
        .join(fixture.name("previous"));
    fs::remove_file(&path).unwrap();
    assert_eq!(fixture.check()["ok"], false);
    fixture.previous["phases"]["release"] =
        json!({"status":"complete","completed_at":"2026-09-04T00:00:00Z"});
    fixture.publish();
    assert_eq!(
        fixture.check()["error"]["message"],
        "reset 上一版账本没有证明最终 release 的直接前像"
    );
    fixture.previous = fixture.ledger.clone();
    fixture.previous["phases"]["release"] = json!({"status":"running","completed_at":null});
    fixture.previous["resources"]["database"]["status"] = json!("private-secret");
    fixture.publish();
    let response = fixture.check();
    assert_eq!(
        response["error"]["message"],
        "reset 上一版账本不属于最终账本的直接前像"
    );
    assert!(!response.to_string().contains("private-secret"));
}

#[test]
fn reset_plan_process_binds_original_document_bytes_instead_of_equivalent_json() {
    let fixture = Fixture::new();
    let document = "{\"environment\": \"test\", \"label\": \"中文 空格\"}";
    let manifest: Value = serde_json::from_str(document).unwrap();
    let hash = hex::encode(Sha256::digest(document.as_bytes()));
    let plan = json!({"manifest":manifest,"plan_hash":hash});
    let path = fixture.directory.path().join("reset-plan.command.json");
    let receipt = json!({"command":["D:/中文 空格/reset.exe","plan"],"returncode":0,"error_type":null,
        "stdout":format!("{document}\nplan_hash={hash}\n"),"stderr":""});
    fixture.write("reset-plan.command.json", &receipt);
    let check = || {
        fixture.invoke(
            "reset-plan",
            json!({"path":path,"executable":"D:/中文 空格/reset.exe","plan":plan}),
        )
    };
    assert_eq!(check()["result"], binding(&path));
    let mut changed = receipt.clone();
    changed["stdout"] = json!(receipt["stdout"].as_str().unwrap().replace('\n', "\r\n"));
    fixture.write("reset-plan.command.json", &changed);
    assert_eq!(check()["ok"], true);
    changed["stdout"] = json!(
        receipt["stdout"]
            .as_str()
            .unwrap()
            .replace("\"environment\": \"test\"", "\"environment\" : \"test\"")
    );
    fixture.write("reset-plan.command.json", &changed);
    assert_eq!(
        check()["error"]["message"],
        "reset plan 命令收据与固定清单不一致"
    );
}

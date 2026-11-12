use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MARKER: &str = "__source-evidence";

fn invoke(raw: &[u8], arguments: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg(MARKER)
        .args(arguments)
        .env("RYFRAME_RESTORE_SOURCE_PROTOCOL", "private-do-not-read")
        .env("APP_ENV", "prod")
        .env("APP_CONFIG_DIR", "absent-legacy-directory")
        .env("APP_DATABASE_PASSWORD", "poisoned-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn malformed_secret_bearing_requests_have_sanitized_failure_and_no_retry_permission() {
    for raw in [
        br#"{"operation":"private-secret-marker" invalid}"#.as_slice(),
        br#"{"operation":"cache-plan","write":false,"arguments":{"namespace":"private-secret-marker","subjects":[]}}"#,
        br#"{"operation":"result","write":false,"arguments":{"value":"private-secret-marker"}}"#,
    ] {
        let output = invoke(raw, &[]);
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-secret-marker"));
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["remote_write_may_have_occurred"], false);
        assert_eq!(response["error"]["automatic_retry_permitted"], false);
    }
    assert!(!invoke(b"{}", &["--unexpected"]).status.success());
}

#[test]
fn retired_recovery_operations_cannot_publish_intent_or_probe_a_listener() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/source-evidence-process");
    fs::create_dir_all(&base).unwrap();
    let coordinator = tempfile::tempdir_in(base).unwrap();
    let local = coordinator.path().join(".local-tests");
    fs::create_dir(&local).unwrap();
    let root = local.join("runtime");
    let intent = local.join("runtime.runtime-create.json");
    let descriptor =
        |name: &str| json!({"path":local.join(name),"bytes":2,"sha256":"a".repeat(64)});
    let binding = json!({"registration":descriptor("registration.json"),
        "target_plan":descriptor("target-plan.json")});
    let tool = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let mut generation = json!({"operation":"runtime-generation","write":false,"arguments":{
        "action":"new-generation","root":root,"binding":binding,"previous":null,"number":1,
        "commands":{"api":[tool],"worker":[tool],"frontend":[tool]},
        "value":{"registration":binding,"runtime_directory":root,
            "tools":{"coordinator_root":coordinator.path()},"authority":{"scope_id":"test-runtime"}}
    }});
    assert_rejected_operation(&generation, "来源证据操作无效");
    generation["write"] = json!(true);
    generation["arguments"]["action"] = json!("create-intent");
    generation["arguments"]["commands"] = Value::Null;
    assert_rejected_operation(&generation, "来源证据写入授权与操作不一致");
    assert!(!intent.exists() && !root.exists());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let probe = json!({"operation":"runtime-probe","write":false,"arguments":{
        "role":"worker","url":format!("http://{}/readyz", listener.local_addr().unwrap()),
    }});
    assert_rejected_operation(&probe, "来源证据操作无效");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    let frontend = json!({"operation":"frontend-build-validation","write":false,"arguments":{
        "action":"artifacts","root":coordinator.path(),"value":null,
        "expected_head":null,"inventory":null,
    }});
    assert_rejected_operation(&frontend, "来源证据操作无效");
}

fn assert_rejected_operation(request: &Value, diagnostic: &str) {
    let output = invoke(&serde_json::to_vec(request).unwrap(), &[]);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["remote_write_may_have_occurred"], false);
    assert_eq!(value["error"]["automatic_retry_permitted"], false);
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains(diagnostic), "{message}");
}

#[test]
fn retired_reference_dataset_operations_are_rejected_by_the_native_protocol() {
    for (operation, arguments) in [
        (
            "reference-dataset-protocol",
            json!({"backend":"private-test-path","plan":"private-test-plan","preflight":"private-test-preflight"}),
        ),
        (
            "reference-dataset-timeout",
            json!({"plan":{"dataset":{"timeout_seconds":21600}}}),
        ),
        (
            "reference-dataset-preflight",
            json!({"plan":{"source":{"scope_id":"source"}}}),
        ),
        (
            "reference-dataset-result",
            json!({"preflight":{},"actual_preflight":{},"source":{},"actual_source":{},"tools":{},"actual_tools":{},"result":{}}),
        ),
    ] {
        assert_rejected_operation(
            &json!({"operation":operation,"write":false,"arguments":arguments}),
            "参考证据操作无效",
        );
    }
}

#[test]
fn reference_operations_are_strict_read_only_or_explicit_local_writes() {
    let hash = invoke(
        &serde_json::to_vec(&json!({
            "operation":"reference-plan-hash","write":false,
            "arguments":{"value":{"中文":"值","a":1}}
        }))
        .unwrap(),
        &[],
    );
    assert!(hash.status.success());
    let response: Value = serde_json::from_slice(&hash.stdout).unwrap();
    let expected = hex::encode(Sha256::digest(
        serde_json::to_vec(&json!({"中文": "值", "a": 1})).expect("规范 JSON 应可序列化"),
    ));
    assert_eq!(response["result"]["value"], expected);

    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join("raw.sql");
    let output = directory.path().join("normalized.sql");
    fs::write(
        &raw,
        "/*!40101 SET @OLD=1 */;\nINSERT INTO `sys_post` (`id`) VALUES (1);\n",
    )
    .unwrap();
    let request = json!({
        "operation":"reference-sql-normalize","write":true,
        "arguments":{"raw":raw,"output":output,"tables":["sys_post"]}
    });
    let normalized = invoke(&serde_json::to_vec(&request).unwrap(), &[]);
    assert!(normalized.status.success());
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "-- ryframe-reference-data-v1\nINSERT INTO `sys_post` (`id`) VALUES (1);\n"
    );
    let mut wrong_write = request;
    wrong_write["write"] = Value::Bool(false);
    assert!(
        !invoke(&serde_json::to_vec(&wrong_write).unwrap(), &[])
            .status
            .success()
    );
    assert!(
        !invoke(
            &serde_json::to_vec(&json!({
                "operation":"reference-plan-hash","write":false,
                "arguments":{"value":{},"unknown":true}
            }))
            .unwrap(),
            &[],
        )
        .status
        .success()
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn staging_git_reads_ignore_inherited_repository_object_and_replacement_overrides() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/source-evidence-process");
    fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("openapi")).unwrap();
    fs::create_dir_all(root.join(".local-tests/run/verification")).unwrap();
    fs::write(
        root.join("openapi/openapi.json"),
        "{\"openapi\":\"3.1.0\"}\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), ".local-tests/\n").unwrap();
    git(root, &["init", "--quiet"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Source test",
            "-c",
            "user.email=source@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    let sha = git(root, &["rev-parse", "HEAD"]);
    let tool = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let request = json!({"operation":"staging-create","write":true,"arguments":{
        "coordinator":root,"execution":root,"directory":root.join(".local-tests/run/verification"),
        "coordinator_source":{"snapshot":{"head":sha,"patch_sha256":"a".repeat(64),"files":[],"clean":true},
            "worktree_fingerprint":format!("sha256:{}","a".repeat(64)),"fingerprints":{
                "product":{"sha256":"a".repeat(64),"files":1},
                "test_tools":{"sha256":"a".repeat(64),"files":0},
                "support":{"sha256":"a".repeat(64),"files":1}}},
        "execution_sha":sha,"tool":{"path":tool,"sha256":hex::encode(Sha256::digest(fs::read(tool).unwrap()))}}});
    let mut child = Command::new(tool)
        .arg(MARKER)
        .env("GIT_DIR", root.join("absent"))
        .env("GIT_OBJECT_DIRECTORY", root.join("absent-objects"))
        .env("GIT_REPLACE_REF_BASE", "refs/replace/hostile")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        fs::read(root.join(".local-tests/run/verification/source-tools/openapi/openapi.json"))
            .unwrap(),
        b"{\"openapi\":\"3.1.0\"}\n"
    );
}

#[test]
fn physical_binding_reads_selected_plaintext_files_and_ignores_legacy_environment() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/source-binding-process");
    fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    let root = temporary.path();
    fs::create_dir(root.join(".local-tests")).unwrap();
    let directory = root.join(".local-tests/中文 隔离配置");
    fs::create_dir(&directory).unwrap();
    let defaults = root.join(".local-tests/client.cnf");
    fs::write(&defaults,"[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=private-secret\nssl-mode=REQUIRED\n").unwrap();
    let configuration = "scope_id='source-process'\n[database.primary]\nhost='127.0.0.1'\nport=3306\ndatabase='fixture'\nusername='fixture'\npassword='private-secret'\ntls_mode='required'\n[object_storage]\nbackend='rustfs'\nendpoint='http://127.0.0.1:29000'\nregion='us-east-1'\naccess_key='private-access'\nsecret_key='private-object'\nuse_ssl=false\n";
    let file = directory.join("app.test.toml");
    fs::write(&file, configuration).unwrap();
    let bind_configuration = || {
        let request = json!({"operation":"reference-configuration-binding","write":false,
            "arguments":{"backend":root,"binding":null,"directory":directory,"app_env":"test"}});
        let output = invoke(&serde_json::to_vec(&request).unwrap(), &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        response["result"]["value"].clone()
    };
    let mut request = json!({"operation":"physical-binding","write":false,"arguments":{
        "backend":root,"evidence_root":root,"config_dir":directory,"app_env":"test",
        "source":{"scope_id":"source-process","databases":[{"key":"shared-control","kind":"combined","mode":"shared","database":"fixture","server_uuid":"server-fixture","defaults_file":defaults,"defaults_sha256":hex::encode(Sha256::digest(fs::read(&defaults).unwrap()))}],
        "s3":{"configuration":bind_configuration()}}}});
    let output = invoke(&serde_json::to_vec(&request).unwrap(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"]["value"]["databases"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for secret in [
        "private-secret",
        "private-access",
        "private-object",
        "poisoned-secret",
    ] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), configuration);
    assert!(!directory.join("app.toml").exists());
    let mut retired = request.clone();
    retired["arguments"]["source"]["s3"] = json!({
        "endpoint":"http://127.0.0.1:29000","region":"us-east-1",
        "access_key":"private-access","secret_key":"private-object",
    });
    assert_rejected_operation(&retired, "来源对象文件绑定字段缺失或包含未知字段");
    fs::write(
        &file,
        configuration.replace("source-process", "other-scope"),
    )
    .unwrap();
    request["arguments"]["source"]["s3"]["configuration"] = bind_configuration();
    assert_rejected_operation(&request, "目标环境与计划 scope 必须明确一致");
    fs::write(&file, "password='private-secret' invalid").unwrap();
    let failure = invoke(&serde_json::to_vec(&request).unwrap(), &[]);
    assert!(!failure.status.success());
    assert!(!String::from_utf8_lossy(&failure.stdout).contains("private-secret"));
}

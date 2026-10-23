use std::{
    fs,
    io::Write,
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

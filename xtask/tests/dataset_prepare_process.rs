use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-dataset-prepare-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.directory.join(name);
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn invoke(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .output()
        .unwrap()
}

fn request(plan: &std::path::Path, preflight: &std::path::Path) -> Vec<String> {
    vec![
        "check".into(),
        "recovery".into(),
        "dataset-prepare".into(),
        "--plan".into(),
        plan.to_string_lossy().into_owned(),
        "--preflight".into(),
        preflight.to_string_lossy().into_owned(),
        "--write".into(),
    ]
}

#[test]
fn invalid_requests_exit_two_before_starting_any_child() {
    let fixture = Fixture::new();
    let plan = fixture.file("plan.json", b"{}\n");
    let preflight = fixture.file("preflight.json", b"{}\n");
    let valid = request(&plan, &preflight);
    for arguments in [
        valid[..valid.len() - 1].to_vec(),
        [valid.clone(), vec!["--unknown".into()]].concat(),
        vec![
            "check".into(),
            "recovery".into(),
            "dataset-prepare".into(),
            "--plan".into(),
            "relative.json".into(),
            "--preflight".into(),
            preflight.to_string_lossy().into_owned(),
            "--write".into(),
        ],
    ] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ node"));
    }
}

#[test]
fn valid_request_uses_native_validation_without_child_or_logged_arguments() {
    let fixture = Fixture::new();
    let plan = fixture.file("计划.json", b"{}\n");
    let preflight = fixture.file("预检.json", b"{}\n");
    let result = invoke(&request(&plan, &preflight));
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!stdout.contains("→ node"), "{stdout}");
    assert!(!stdout.contains("--plan"), "{stdout}");
    assert!(!stdout.contains(plan.to_str().unwrap()), "{stdout}");
    assert!(stderr.contains("任务失败"), "{stderr}");
}

#[test]
fn private_stdin_rejects_oversize_duplicates_extra_argv_and_never_writes() {
    let fixture = Fixture::new();
    for (arguments, input) in [
        (vec!["__recovery-dataset"], b"{}".to_vec()),
        (
            vec!["__recovery-dataset"],
            b"{\"write\":true,\"write\":false}".to_vec(),
        ),
        (vec!["__recovery-dataset"], vec![b'x'; 32769]),
        (vec!["__recovery-dataset"], vec![255]),
        (vec!["__recovery-dataset", "untrusted-secret"], Vec::new()),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&input);
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("untrusted-secret"));
        assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 0);
    }
}

#[test]
fn bound_s3_configuration_errors_never_expose_file_credentials() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let directory = fixture.directory.join("config");
    fs::create_dir(&directory).unwrap();
    let bytes=b"[bootstrap]\nadmin_password='private-admin-value'\n[object_storage]\naccess_key='private-access-value'\nsecret_key='private-secret-value'\nsession_token='private-token-value'\n";
    fs::write(directory.join("app.test.toml"), bytes).unwrap();
    let mut binding = json!({"app_env":"test","directory":directory,"files":{"app.test.toml":{"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))}}});
    binding["sha256"] = json!(hex::encode(Sha256::digest(
        serde_json::to_vec(&binding).unwrap()
    )));
    let request = json!({"format_version":1,"kind":"ryframe-native-s3-read","scope_id":"source","configuration_binding":{"endpoint":"http://127.0.0.1:9","region":"us-east-1","configuration":binding},"bucket":"uploads","operation":"head-object","key":"source/file","if_match":null,"version":null,"destination":null,"max_bytes":0});
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__s3-read")
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
    assert_eq!(output.status.code(), Some(1));
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in [
        "private-admin-value",
        "private-access-value",
        "private-secret-value",
        "private-token-value",
    ] {
        assert!(!serde_json::to_string(&request).unwrap().contains(secret));
        assert!(!diagnostic.contains(secret));
    }
    assert!(diagnostic.contains("会话令牌"), "{diagnostic}");
    let mut inline = request.clone();
    let binding = inline
        .as_object_mut()
        .unwrap()
        .remove("configuration_binding")
        .unwrap();
    inline["configuration"] = binding;
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__s3-read")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&inline).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let diagnostic = String::from_utf8_lossy(&output.stdout);
    assert!(diagnostic.contains("字段缺失"), "{diagnostic}");
    for secret in [
        "private-admin-value",
        "private-access-value",
        "private-secret-value",
        "private-token-value",
    ] {
        assert!(!diagnostic.contains(secret));
    }
}

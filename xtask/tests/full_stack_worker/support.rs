use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
pub fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn encode(value: &Value, spaced: bool) -> String {
    let text = serde_json::to_string(value).unwrap();
    let mut output = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in text.chars() {
        if c == '"' && !escaped {
            quoted = !quoted;
        }
        if !c.is_ascii() {
            for unit in c.encode_utf16(&mut [0; 2]) {
                use std::fmt::Write;
                write!(output, "\\u{unit:04x}").unwrap();
            }
        } else {
            output.push(c);
        }
        if spaced && !quoted && matches!(c, ',' | ':') {
            output.push(' ');
        }
        escaped = quoted && c == '\\' && !escaped;
    }
    output
}
pub fn hash(value: &Value) -> String {
    sha(encode(value, false).as_bytes())
}
fn canonical(path: &Path) -> PathBuf {
    let path = fs::canonicalize(path).unwrap();
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
    }
    #[cfg(not(windows))]
    {
        path
    }
}
fn executable() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let directory = tempfile::tempdir_in(local()).unwrap();
            let executable = directory
                .path()
                .join(format!("worker 替身{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new("rustc");
            command
                .args([
                    "--edition=2024",
                    "--crate-name=worker_fixture",
                    "-C",
                    "debuginfo=0",
                ])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/full_stack_worker.rs"),
                )
                .arg("-o")
                .arg(&executable);
            if cfg!(windows) {
                command.args(["-C", "linker=rust-lld.exe"]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (directory, canonical(&executable))
        })
        .1
}
fn local() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/worker-native-tests");
    fs::create_dir_all(&path).unwrap();
    path
}
pub fn read(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
pub fn write(path: impl AsRef<Path>, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
pub fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate() {
        assert!(Instant::now() < deadline, "等待进程证据超时");
        thread::sleep(Duration::from_millis(20));
    }
}
pub struct Fixture {
    pub temporary: Option<tempfile::TempDir>,
    pub backend: PathBuf,
    pub run: PathBuf,
    pub port: u16,
    pub binary: PathBuf,
}
impl Fixture {
    pub fn new(mode: &str) -> Self {
        let temporary = tempfile::tempdir_in(local()).unwrap();
        let backend = canonical(temporary.path());
        let run = backend.join("runtime 空格");
        fs::create_dir(&run).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let binary = backend.join(format!("worker{}", std::env::consts::EXE_SUFFIX));
        fs::copy(executable(), &binary).unwrap();
        let mut digests = vec![];
        for (name, id) in [("config", 3), ("worker-config", 2)] {
            let directory = run.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("app.toml"), "").unwrap();
            fs::write(directory.join("app.test.toml"),format!("scope_id=\"worker-native-tests\"\nsnowflake_worker_id={id}\n[app]\nport=8080\n[jobs]\nmode=\"external\"\nhealth_host=\"127.0.0.1\"\nhealth_port={port}\n")).unwrap();
            let files = json!({"app.toml":sha(b""),"app.test.toml":sha(&fs::read(directory.join("app.test.toml")).unwrap())});
            digests.push(sha(encode(&json!({"environment":{"APP_ENV":"test"},"config_directory":directory,"files":files}),true).as_bytes()));
        }
        write(
            run.join("binaries.json"),
            &json!({"ryframe":binary,"ryframe-worker":binary,"ryframe-reset":binary,"ryframe-migrate":binary}),
        );
        let artifact = json!({"path":binary,"sha256":sha(&fs::read(&binary).unwrap())});
        write(
            run.join("runtime.json"),
            &json!({"format_version":1,"backend_root":backend,"scope_id":"worker-native-tests","api_config_dir":run.join("config"),"worker_config_dir":run.join("worker-config"),"api_configuration_sha256":digests[0],"worker_configuration_sha256":digests[1],"configuration_sha256":sha(encode(&json!(digests),true).as_bytes()),"worker_ready_url":format!("http://127.0.0.1:{port}/readyz"),"artifacts":{"api":artifact,"worker":artifact}}),
        );
        fs::write(run.join("mode"), mode).unwrap();
        Self {
            temporary: Some(temporary),
            backend,
            run,
            port,
            binary,
        }
    }
    pub fn command(&self, action: &str, timeout: f64) -> Child {
        let input = json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"control","arguments":{"action":action,"backend":self.backend,"directory":self.run,"timeout":timeout}});
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("__full-stack-worker")
            .env("APP_ENV", "test")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&input).unwrap())
            .unwrap();
        child
    }
    pub fn control(&self, action: &str, timeout: f64) -> (bool, Value) {
        let output = self.command(action, timeout).wait_with_output().unwrap();
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            output.status.success(),
            serde_json::from_slice(&output.stdout).unwrap(),
        )
    }
    pub fn ok(&self, action: &str) -> Value {
        let (ok, value) = self.control(action, 5.0);
        assert!(ok, "{value}; evidence={:?}", self.run);
        value["result"].clone()
    }
    pub fn tree(&self) -> Value {
        read(self.run.join("worker-tree.json"))
    }
    pub fn archive(&self, id: &Value) -> PathBuf {
        self.run
            .join("worker-control-receipts")
            .join(id.as_str().unwrap())
    }
    pub fn gone(&self, pid: u32) {
        wait(|| !super::process_liveness::process_is_running(pid));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Some(temporary) = self.temporary.take() {
                let _ = temporary.keep();
            }
            return;
        }
        if !std::thread::panicking() {
            let _ = self.control("stop", 3.0);
        }
        // 失败诊断保留小收据；绝不从测试之外推断待回收资源。
        if self.run.join("worker-tree.json").exists() {
            let tree = self.tree();
            let control = self.run.join(format!(
                "worker-tree-{}-control.json",
                tree["operation_id"].as_str().unwrap()
            ));
            if !control.exists() {
                write(
                    control,
                    &json!({"format_version":1,"kind":"full-stack-process-tree-control","operation_id":tree["operation_id"],"mode":"crash","requested_at_ns":1}),
                );
            }
            for name in ["process", "supervisor", "monitor"] {
                let pid = tree[name]["pid"].as_u64().unwrap() as u32;
                wait(|| !super::process_liveness::process_is_running(pid));
            }
        }
    }
}

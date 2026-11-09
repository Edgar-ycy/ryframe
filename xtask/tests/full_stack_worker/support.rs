use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{OnceLock, mpsc},
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

fn controller_exited(
    fixture: &Path,
    controller: &mut Child,
    status: std::process::ExitStatus,
) -> ! {
    let mut report = json!({"pid":controller.id(),"status":status.to_string(),
        "exit_code":status.code(),"success":status.success(),"stdout":{"received":false},"stderr":{"received":false}});
    let streams = [
        (
            "stdout",
            controller
                .stdout
                .take()
                .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        ),
        (
            "stderr",
            controller
                .stderr
                .take()
                .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        ),
    ];
    let (sender, receiver) = mpsc::channel();
    let mut pending = 0;
    for (name, stream) in streams {
        let Some(stream) = stream else {
            report[name] = json!({"missing":true});
            continue;
        };
        pending += 1;
        let sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = vec![];
            let error = stream
                .take(65537)
                .read_to_end(&mut bytes)
                .err()
                .map(|error| format!("{:?}", error.kind()));
            let overflow = bytes.len() > 65536;
            bytes.truncate(65536);
            let _ = sender.send((name, bytes, overflow, error));
        });
    }
    drop(sender);
    let until = Instant::now() + Duration::from_secs(2);
    for _ in 0..pending {
        let Ok((name, bytes, overflow, error)) =
            receiver.recv_timeout(until.saturating_duration_since(Instant::now()))
        else {
            break;
        };
        let path = fixture.join(format!("start-controller-{name}.log"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        report[name] = json!({"received":true,"path":path,"captured_bytes":bytes.len(),"overflow":overflow,"read_error":error});
    }
    let path = fixture.join("start-controller-exit.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    serde_json::to_writer_pretty(&mut file, &report).unwrap();
    file.write_all(b"\n").unwrap();
    file.sync_all().unwrap();
    panic!(
        "启动控制器在产品启动标记前提前退出（{status}）；保留诊断 {}",
        path.display()
    );
}

#[derive(Clone, Copy)]
pub enum Role {
    Api,
    Worker,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
        }
    }
}

pub struct Fixture {
    pub temporary: Option<tempfile::TempDir>,
    pub backend: PathBuf,
    pub run: PathBuf,
    pub port: u16,
    pub api_port: u16,
    pub binary: PathBuf,
    role: Role,
}
impl Fixture {
    pub fn new(mode: &str) -> Self {
        Self::with_role(mode, Role::Worker)
    }
    pub fn with_role(mode: &str, role: Role) -> Self {
        let temporary = tempfile::tempdir_in(local()).unwrap();
        let backend = canonical(temporary.path());
        let run = backend.join("runtime 空格");
        fs::create_dir(&run).unwrap();
        let worker_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = worker_listener.local_addr().unwrap().port();
        let api_listener = match role {
            Role::Api => Some(std::net::TcpListener::bind("127.0.0.1:0").unwrap()),
            Role::Worker => None,
        };
        let api_port = api_listener
            .as_ref()
            .map_or(8080, |listener| listener.local_addr().unwrap().port());
        drop(worker_listener);
        let binary = backend.join(format!("{}{}", role.name(), std::env::consts::EXE_SUFFIX));
        fs::copy(executable(), &binary).unwrap();
        let api = binary.clone();
        let worker = match role {
            Role::Api => {
                let path = backend.join(format!("worker{}", std::env::consts::EXE_SUFFIX));
                fs::copy(executable(), &path).unwrap();
                path
            }
            Role::Worker => binary.clone(),
        };
        let app = match role {
            Role::Api => format!("[app]\nhost=\"127.0.0.1\"\nport={api_port}\n"),
            Role::Worker => "[app]\nport=8080\n".into(),
        };
        let mut digests = vec![];
        for (name, id) in [("config", 3), ("worker-config", 2)] {
            let directory = run.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("app.toml"), "").unwrap();
            fs::write(directory.join("app.test.toml"),format!("scope_id=\"worker-native-tests\"\nsnowflake_worker_id={id}\n{app}[jobs]\nmode=\"external\"\nhealth_host=\"127.0.0.1\"\nhealth_port={port}\n")).unwrap();
            let files = json!({"app.toml":sha(b""),"app.test.toml":sha(&fs::read(directory.join("app.test.toml")).unwrap())});
            digests.push(sha(encode(&json!({"environment":{"APP_ENV":"test"},"config_directory":directory,"files":files}),true).as_bytes()));
        }
        write(
            run.join("binaries.json"),
            &json!({"ryframe":api,"ryframe-worker":worker,"ryframe-reset":binary,"ryframe-migrate":binary}),
        );
        let api_artifact = json!({"path":api,"sha256":sha(&fs::read(&api).unwrap())});
        let worker_artifact = json!({"path":worker,"sha256":sha(&fs::read(&worker).unwrap())});
        write(
            run.join("runtime.json"),
            &json!({"format_version":1,"backend_root":backend,"scope_id":"worker-native-tests","api_config_dir":run.join("config"),"worker_config_dir":run.join("worker-config"),"api_configuration_sha256":digests[0],"worker_configuration_sha256":digests[1],"configuration_sha256":sha(encode(&json!(digests),true).as_bytes()),"worker_ready_url":format!("http://127.0.0.1:{port}/readyz"),"artifacts":{"api":api_artifact,"worker":worker_artifact}}),
        );
        fs::write(run.join("mode"), mode).unwrap();
        Self {
            temporary: Some(temporary),
            backend,
            run,
            port,
            api_port,
            binary,
            role,
        }
    }
    pub fn command(&self, action: &str, timeout: f64) -> Child {
        let (entry, kind, operation) = match self.role {
            Role::Worker => (
                "__full-stack-worker",
                "ryframe-full-stack-worker",
                "control",
            ),
            Role::Api => (
                "__full-stack-process",
                "ryframe-full-stack-process",
                "role-control",
            ),
        };
        let mut arguments =
            json!({"action":action,"backend":self.backend,"directory":self.run,"timeout":timeout});
        if let Role::Api = self.role {
            arguments["role"] = json!("api");
        }
        let input =
            json!({"format_version":1,"kind":kind,"operation":operation,"arguments":arguments});
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg(entry)
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
    pub fn wait_started(&self, controller: &mut Child) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let marker = self.run.join(format!("{}-started", self.role.name()));
        while !marker.exists() {
            if let Some(status) = controller.try_wait().unwrap() {
                controller_exited(&self.backend, controller, status);
            }
            assert!(Instant::now() < deadline, "等待进程证据超时");
            thread::sleep(Duration::from_millis(20));
        }
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
        read(self.run.join(format!("{}-tree.json", self.role.name())))
    }
    pub fn archive(&self, id: &Value) -> PathBuf {
        self.run
            .join(format!("{}-control-receipts", self.role.name()))
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
        if self
            .run
            .join(format!("{}-tree.json", self.role.name()))
            .exists()
        {
            let tree = self.tree();
            let control = self.run.join(format!(
                "{}-tree-{}-control.json",
                self.role.name(),
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

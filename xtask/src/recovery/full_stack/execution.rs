use super::{Port, context::Context};
use crate::{Result, ci::databases::snapshot::Frozen};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Native<'a> {
    context: &'a Context,
    cancel: Arc<AtomicBool>,
    api: Option<Value>,
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/full_stack/process_tests.rs"]
mod process_tests;

impl<'a> Native<'a> {
    pub fn new(context: &'a Context) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(async {
                    let _ = tokio::signal::ctrl_c().await;
                    trigger.store(true, Ordering::Release);
                });
            }
        });
        Ok(Self {
            context,
            cancel,
            api: None,
        })
    }

    fn support(&mut self, operation: &str, arguments: Value) -> Result<Value> {
        if matches!(operation, "worker-start" | "worker-stop") {
            return self.worker_control(operation);
        }
        let executable = std::env::current_exe()?;
        let bytes = fs::read(&executable)?;
        let tool = json!({"format_version":1,"kind":"ryframe-native-rate-limit-tool","path":executable,
            "bytes":bytes.len(),"sha256":crate::devex::runtime_native::artifacts::sha(&bytes)});
        let input = serde_json::to_vec(
            &json!({"format_version":1,"operation":operation,"backend":self.context.root,
            "directory":self.context.output,"tool":tool,"arguments":arguments}),
        )?;
        if input.len() > 32768 {
            return Err("全栈底层请求超过32KiB".into());
        }
        let mut command = Command::new(crate::process::resolved_executable(
            &self.context.root,
            "python",
        )?);
        command.args([
            "-X",
            "utf8",
            "-E",
            "-s",
            "-B",
            "tools/python/full_stack_ci_support.py",
        ]);
        self.environment(&mut command);
        command.env("APP_ENV", "test");
        // 既有持久 supervisor 必须继续脱离一次性调用者的 Job 生命周期。
        let persistent = operation == "api-launch";
        let cleanup = operation.ends_with("-stop");
        let output = capture(
            command,
            &self.context.root,
            &input,
            Duration::from_secs(210),
            (!cleanup).then_some(&self.cancel),
            persistent,
            false,
        )?;
        let value = crate::recovery::clone::native::json::parse_fragment(&output, "全栈底层响应")
            .map_err(|_| "全栈底层返回无效JSON；保留收据，不重放")?;
        if value["ok"] != true
            || value
                .as_object()
                .is_none_or(|v| v.len() != 2 || !v.contains_key("result"))
        {
            return Err(format!(
                "全栈底层 {operation} 未确认完成；保留来源、进程及操作收据，不重放"
            )
            .into());
        }
        Ok(value["result"].clone())
    }

    fn worker_control(&mut self, operation: &str) -> Result<Value> {
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("__full-stack-worker");
        self.environment(&mut command);
        command.env("APP_ENV", "test");
        let request = serde_json::to_vec(
            &json!({"format_version":1,"kind":"ryframe-full-stack-worker","operation":"control","arguments":{"backend":self.context.root,"directory":self.context.output,"action":operation.trim_start_matches("worker-"),"timeout":180}}),
        )?;
        let output = capture(
            command,
            &self.context.root,
            &request,
            Duration::from_secs(210),
            (operation != "worker-stop").then_some(&self.cancel),
            true,
            false,
        )?;
        let value =
            crate::recovery::clone::native::json::parse_fragment(&output, "Worker 原生控制响应")?;
        if value["ok"] != true
            || value
                .as_object()
                .is_none_or(|v| v.len() != 2 || !v.contains_key("result"))
        {
            return Err("Worker 原生控制未确认；保留收据，不自动重放".into());
        }
        Ok(value["result"].clone())
    }

    fn s3_request(&self, marker: &str, request: &Value) -> Result<Value> {
        let mut command = Command::new(std::env::current_exe()?);
        command.arg(marker);
        self.environment(&mut command);
        let output = capture(
            command,
            &self.context.root,
            &serde_json::to_vec(request)?,
            Duration::from_secs(90),
            Some(&self.cancel),
            false,
            false,
        )?;
        let response = crate::recovery::clone::native::json::parse_fragment(&output, "对象响应")?;
        if response["ok"] != true
            || response
                .as_object()
                .is_none_or(|v| v.len() != 2 || !v.contains_key("result"))
        {
            return Err("原生对象操作未确认；保留对象收据，核对远端状态后处理，不自动重放".into());
        }
        Ok(response["result"].clone())
    }

    fn environment(&self, command: &mut Command) {
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if crate::devex::runtime_native::entry::allowed_environment(&name)
                || name.starts_with("DOCKER_")
                || matches!(
                    name.as_ref(),
                    "GITHUB_ACTIONS" | "GITHUB_RUN_ID" | "GITHUB_RUN_ATTEMPT"
                )
            {
                command.env(key, value);
            }
        }
    }
}

impl Port for Native<'_> {
    fn leaf(&mut self, operation: &str) -> Result<Value> {
        if operation == "build" {
            return super::build::run(self.context, self);
        }
        if operation == "api-alive" {
            let identity = self.api.as_ref().ok_or("API 启动未确认创建身份")?;
            let pid = identity["pid"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or("API PID 无效")?;
            if crate::recovery::clone::native::resources::common::process_identity(pid)?.as_ref()
                != Some(identity)
            {
                return Err("API 已退出或创建身份漂移，停止等待并按原收据核对".into());
            }
            return Ok(json!({"state":"running"}));
        }
        let value = self.support(operation, json!({}))?;
        if operation == "api-launch" {
            self.api = Some(
                crate::recovery::clone::native::resources::common::validate_process_identity(
                    &value["identity"],
                )?,
            );
        }
        Ok(value)
    }
    fn command(&mut self, program: &str, args: &[String], log: Option<&Path>) -> Result<String> {
        self.command_at(&self.context.root.clone(), program, args, log)
    }
    fn command_at(
        &mut self,
        cwd: &Path,
        program: &str,
        args: &[String],
        log: Option<&Path>,
    ) -> Result<String> {
        let actual = if cfg!(windows) && program == "corepack" {
            "corepack.cmd"
        } else {
            program
        };
        let mut command = Command::new(actual);
        command.args(args).current_dir(cwd);
        self.environment(&mut command);
        if Path::new(program).is_absolute() {
            command.env("APP_ENV", "test");
        }
        let cargo_json =
            program == "cargo" && args.iter().any(|arg| arg == "--message-format=json");
        let output = capture(
            command,
            &self.context.root,
            &[],
            Duration::from_secs(1800),
            Some(&self.cancel),
            false,
            // Cargo JSON 事件与诊断分流，诊断不能混入产物协议。
            !cargo_json,
        );
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                if let Some(path) = log {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?;
                    writeln!(file, "{error}")?;
                }
                return Err(error);
            }
        };
        if cargo_json && log.is_none() {
            // 私有产物协议不写日志，路径字节在解析前不能被脱敏替换。
            return Ok(output);
        }
        let output = match self.context.table() {
            Ok(table) => redact(output, &table),
            Err(_) if log.is_some() => {
                "当前阶段未取得可信文件配置，子进程输出未写入公开日志。\n".into()
            }
            Err(_) => output,
        };
        if let Some(path) = log {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            file.write_all(output.as_bytes())?;
        }
        Ok(output)
    }
    fn ready(&mut self, url: &str) -> Result<bool> {
        crate::http_session::install_crypto_provider()?;
        // 同步阶段中的 HTTP 只访问明确的 loopback readiness，不跟随重定向。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(runtime
            .block_on(async {
                reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(2))
                    .build()?
                    .get(url)
                    .send()
                    .await
                    .map(|v| v.status().is_success())
            })
            .unwrap_or(false))
    }
    fn sleep(&mut self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            if self.cancel.load(Ordering::Acquire) {
                return Err("全栈阶段已取消；保留收据".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    fn storage(&mut self, table: &Value) -> Result<()> {
        let storage = &table["object_storage"];
        let buckets = ["uploads", "avatar", "exports", "imports", "config-packages"];
        if storage["backend"] == "local" {
            let root = Path::new(
                storage["local_base_dir"]
                    .as_str()
                    .ok_or("缺少本地存储目录")?,
            );
            for bucket in buckets {
                fs::create_dir_all(root.join(bucket))?;
            }
        } else {
            let endpoint = url::Url::parse(storage["endpoint"].as_str().ok_or("缺少S3端点")?)?;
            let address = endpoint.socket_addrs(|| {
                Some(if endpoint.scheme() == "https" {
                    443
                } else {
                    80
                })
            })?;
            let started = Instant::now();
            while !address
                .iter()
                .any(|a| std::net::TcpStream::connect_timeout(a, Duration::from_secs(2)).is_ok())
            {
                if started.elapsed() > Duration::from_secs(90) {
                    return Err("隔离对象存储就绪超时".into());
                }
                self.sleep(Duration::from_secs(1))?;
            }
            let path = self.context.output.join("ci-storage.json");
            let mut receipt =
                json!({"format_version":1,"buckets":[],"automatic_retry_permitted":false});
            crate::recovery::clone::native::json::write_new(&path, &receipt)?;
            for (index, bucket) in buckets.iter().enumerate() {
                receipt["buckets"]
                    .as_array_mut()
                    .ok_or("对象收据无效")?
                    .push(json!({"bucket":bucket,"state":"creation-unknown"}));
                super::context::journal(&path, &receipt)?;
                let configuration = json!({"endpoint":storage["endpoint"],"region":storage["region"],
                    "access_key":storage["access_key"],"secret_key":storage["secret_key"]});
                let mut request = json!({"format_version":1,"kind":"ryframe-native-s3-create-bucket",
                    "scope_id":table["scope_id"],"configuration":configuration,"bucket":bucket});
                let result = self.s3_request("__s3-create-bucket", &request)?;
                if result["status"] != "created_unverified" {
                    return Err("对象桶创建未确认，保留 ci-storage.json，禁止自动重放".into());
                }
                request["kind"] = json!("ryframe-native-s3-inspect");
                request["operation"] = json!("head-bucket");
                request["prefix"] = Value::Null;
                request["continuation"] = Value::Null;
                self.s3_request("__s3-inspect", &request)?;
                receipt["buckets"][index]["state"] = json!("created");
                super::context::journal(&path, &receipt)?;
            }
        }
        Ok(())
    }
    fn databases(&mut self, frozen: &Frozen) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let cancel = crate::devex::runtime_native::process::Cancellation::default();
        let trigger = cancel.clone();
        let flag = self.cancel.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let monitor = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                if flag.load(Ordering::Acquire) {
                    trigger.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let result = runtime.block_on(frozen.prepare(&cancel));
        finished.store(true, Ordering::Release);
        let _ = monitor.join();
        result.map(|_| ())
    }
}

fn capture(
    mut command: Command,
    cwd: &Path,
    input: &[u8],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    persistent: bool,
    merge_stderr: bool,
) -> Result<String> {
    // 请求可能包含 S3 明文配置；临时 stdin 文件也必须位于受限目录中。
    let temporary = tempfile::tempdir_in(cwd.join(".local-tests"))?;
    let private = temporary.path().join("stdin");
    crate::ci::databases::files::private_directory(&private)
        .map_err(|code| format!("无法准备受限全栈输入目录：{code}；未启动子进程"))?;
    let mut file = tempfile::tempfile_in(private)?;
    file.write_all(input)?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    if command.get_current_dir().is_none() {
        command.current_dir(cwd);
    }
    command
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = if persistent {
        Child::Persistent(command.spawn()?)
    } else {
        Child::Managed(Box::new(crate::process::ChildGroup::new()?.spawn(command)?))
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let output = reader(
        child.reader(false)?.ok_or("全栈命令缺少 stdout")?,
        overflow.clone(),
    );
    let errors = reader(
        child.reader(true)?.ok_or("全栈命令缺少 stderr")?,
        overflow.clone(),
    );
    let started = Instant::now();
    let result: Result<()> = loop {
        if overflow.load(Ordering::Acquire) {
            break Err("全栈命令输出超过16MiB".into());
        }
        if cancel.is_some_and(|v| v.load(Ordering::Acquire)) {
            break Err("全栈命令取消；保留操作收据，核对未知写入".into());
        }
        if started.elapsed() > timeout {
            break Err("全栈命令超时；保留操作收据，不自动重放".into());
        }
        if let Some(status) = child.waiting()? {
            break if status.success() {
                Ok(())
            } else {
                Err(
                    format!("全栈命令失败，退出状态 {status}；核对阶段收据，不回显子进程内容")
                        .into(),
                )
            };
        }
        thread::sleep(Duration::from_millis(20));
    };
    child.close();
    drop(child);
    let mut bytes = output.join().map_err(|_| "全栈输出线程失败")??;
    let errors = errors.join().map_err(|_| "全栈错误输出线程失败")??;
    result?;
    if merge_stderr {
        bytes.extend(errors);
    }
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("全栈命令输出超过16MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "全栈输出不是UTF-8".into())
}

enum Child {
    Managed(Box<crate::process::ManagedChild>),
    Persistent(std::process::Child),
}
impl Child {
    fn reader(&mut self, stderr: bool) -> Result<Option<Box<dyn Read + Send>>> {
        match self {
            Self::Managed(v) => {
                if stderr {
                    v.take_stderr_reader()
                } else {
                    v.take_stdout_reader()
                }
            }
            Self::Persistent(v) => {
                if stderr {
                    Ok(v.stderr.take().map(|v| Box::new(v) as Box<dyn Read + Send>))
                } else {
                    Ok(v.stdout.take().map(|v| Box::new(v) as Box<dyn Read + Send>))
                }
            }
        }
    }
    fn waiting(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Managed(v) => v.try_wait(),
            Self::Persistent(v) => v.try_wait(),
        }
    }
    fn close(&mut self) {
        if let Self::Persistent(v) = self {
            let _ = v.kill();
            let _ = v.wait();
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn redact(mut text: String, value: &Value) -> String {
    fn collect(value: &Value, values: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if ["password", "secret", "access_key", "token", "private_key"]
                        .iter()
                        .any(|name| key.contains(name))
                        && let Some(secret) = value.as_str().filter(|v| !v.is_empty())
                    {
                        values.push(secret.into());
                    }
                    collect(value, values);
                }
            }
            Value::Array(items) => {
                for value in items {
                    collect(value, values);
                }
            }
            _ => {}
        }
    }
    let mut values = Vec::new();
    collect(value, &mut values);
    values.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for value in values {
        if let Ok(encoded) = serde_json::to_string(&value) {
            text = text.replace(&encoded[1..encoded.len() - 1], "[已脱敏]");
        }
        text = text.replace(&value, "[已脱敏]");
    }
    text
}

fn reader(
    stream: Box<dyn Read + Send>,
    overflow: Arc<AtomicBool>,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes);
        if bytes.len() > 16 * 1024 * 1024 {
            overflow.store(true, Ordering::Release);
        }
        result.map(|_| bytes)
    })
}

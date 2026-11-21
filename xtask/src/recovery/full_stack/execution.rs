use super::{
    Port,
    context::Context,
    storage,
    worker::{self, role::Role},
};
use crate::{
    Result,
    ci::databases::snapshot::Frozen,
    recovery::clone::native::{json as documents, resources::common},
};
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
    #[cfg(test)]
    pub(super) fn for_test(context: &'a Context) -> Self {
        // 个别叶操作的测试无需为测试进程安装全局信号观察。
        Self {
            context,
            cancel: Arc::new(AtomicBool::new(false)),
            api: None,
        }
    }

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

    fn control(&self, role: Role, action: &str) -> Result<Value> {
        let cleanup = matches!(action, "stop" | "stop-strict");
        let cancel = (!cleanup).then_some(self.cancel.as_ref());
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("全栈控制已取消；保留现场，未启动角色控制器".into());
        }
        let mut command = worker::control_command(
            role,
            &self.context.root,
            &self.context.output,
            action,
            180.0,
        )?;
        self.environment(&mut command);
        command.env("APP_ENV", "test");
        let timeout = if role == Role::Api {
            worker::CONTROL_TIMEOUT
        } else {
            Duration::from_secs(210)
        };
        // 持久 supervisor 继续脱离一次性调用者的 Job 生命周期。
        let output = capture_prebound(
            command,
            &self.context.root,
            timeout,
            cancel,
            true,
            if role == Role::Api {
                CaptureOutput::ApiControl
            } else {
                CaptureOutput::Separate
            },
        )?;
        let value = documents::parse_fragment(&output, "全栈角色原生控制响应")?;
        if value["ok"] != true
            || value
                .as_object()
                .is_none_or(|v| v.len() != 2 || !v.contains_key("result"))
        {
            return Err(format!("{} 原生控制未确认；保留收据，不自动重放", role.label()).into());
        }
        Ok(value["result"].clone())
    }

    fn launch_api(&mut self) -> Result<Value> {
        let result = self.control(Role::Api, "start-strict")?;
        if result["operation"] != "start"
            || result["outcome"] != "succeeded"
            || result["state"] != "running"
        {
            return Err("API 原生启动未返回成功运行收据；保留现场".into());
        }
        self.api = Some(common::validate_process_identity(&result["identity"])?);
        Ok(result)
    }

    fn observe_api(&self) -> Result<Value> {
        let identity = self.api.as_ref().ok_or("API 启动未确认创建身份")?;
        if self.cancel.load(Ordering::Acquire) {
            return Err("API 观察已取消；保留收据".into());
        }
        let observed =
            worker::controller::observe(Role::Api, &self.context.root, &self.context.output)?;
        if observed["state"] != "running" || observed.get("identity") != Some(identity) {
            return Err("API 已退出或创建身份漂移；保留原收据".into());
        }
        if self.cancel.load(Ordering::Acquire) {
            return Err("API 观察已取消；保留收据".into());
        }
        Ok(observed)
    }

    fn stop_api(&self) -> Result<Value> {
        if !worker::ci_api_evidence(&self.context.output)? {
            return Ok(json!({"state":"absent"}));
        }
        let result = self.control(Role::Api, "stop-strict")?;
        if result["operation"] != "stop"
            || result["outcome"] != "succeeded"
            || result["state"] != "stopped"
            || result.get("identity") != Some(&Value::Null)
        {
            return Err("API 未返回无存活进程的成功停止收据；保留现场".into());
        }
        Ok(result)
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
                || matches!(
                    name.to_ascii_uppercase().as_str(),
                    "COREPACK_HOME" | "NPM_CONFIG_STORE_DIR" | "PNPM_CONFIG_STORE_DIR"
                )
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
        if operation == "register" {
            return self.context.register_runtime();
        }
        if operation == "build" {
            let root = self.context.root.clone();
            let output = self.context.output.clone();
            return super::build::run(&root, &output, &mut |program, args| {
                self.command(program, args, None)
            });
        }
        match operation {
            "api-launch" => self.launch_api(),
            "api-probe" => self.observe_api(),
            "api-stop" => self.stop_api(),
            "api-port-free" => match self.context.api_url() {
                Ok(url) => worker::wait_api_port_free(&url),
                Err(_) => Ok(json!({"state":"configuration-unavailable"})),
            },
            "worker-start" => self.control(Role::Worker, "start"),
            "worker-stop" => self.control(Role::Worker, "stop"),
            _ => Err("未知全栈控制操作".into()),
        }
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
        if program == "corepack" && args.first().is_some_and(|arg| arg == "pnpm") {
            command.arg("pnpm");
            if let Some(cache) = crate::process::pnpm_cache_argument(&self.context.root)? {
                command.arg(cache);
            }
            command.args(&args[1..]);
        } else {
            command.args(args);
        }
        command.current_dir(cwd);
        self.environment(&mut command);
        if program == "cargo" {
            crate::recovery::clone::native::bridge::cargo_environment::configure_command(
                &self.context.root,
                &mut command,
            )?;
        }
        if program == "corepack" || Path::new(program).is_absolute() {
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
                let error = format!(
                    "程序 {actual} 在目录 {} 执行失败：{error}；先核对该阶段前后像及收据，不直接重放写入命令",
                    cwd.display()
                );
                if let Some(path) = log {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?;
                    writeln!(file, "{error}")?;
                }
                return Err(error.into());
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
    fn storage(&mut self, frozen: &Frozen) -> Result<()> {
        let storage = &frozen.settings()["object_storage"];
        if storage["backend"] == "local" {
            let root = Path::new(
                storage["local_base_dir"]
                    .as_str()
                    .ok_or("缺少本地存储目录")?,
            );
            for bucket in storage::BUCKETS {
                fs::create_dir_all(root.join(bucket))?;
            }
        } else {
            let binding = frozen.object_configuration_binding()?;
            let scope = frozen.scope()?;
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
            storage::prepare_buckets(&self.context.output, &scope, &binding, |marker, request| {
                self.s3_request(marker, request)
            })?;
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

pub(in crate::recovery) fn capture(
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
    command.stdin(Stdio::from(file));
    let output_mode = if merge_stderr {
        CaptureOutput::Merged
    } else {
        CaptureOutput::Separate
    };
    capture_prebound(command, cwd, timeout, cancel, persistent, output_mode)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureOutput {
    Separate,
    Merged,
    ApiControl,
}

fn capture_prebound(
    mut command: Command,
    cwd: &Path,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    persistent: bool,
    output_mode: CaptureOutput,
) -> Result<String> {
    if command.get_current_dir().is_none() {
        command.current_dir(cwd);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
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
    let (result, failed_exit): (Result<()>, bool) = loop {
        if overflow.load(Ordering::Acquire) {
            break (Err("全栈命令输出超过16MiB".into()), false);
        }
        if cancel.is_some_and(|v| v.load(Ordering::Acquire)) {
            break (
                Err("全栈命令取消；保留操作收据，核对未知写入".into()),
                false,
            );
        }
        if started.elapsed() > timeout {
            break (Err("全栈命令超时；保留操作收据，不自动重放".into()), false);
        }
        if let Some(status) = child.waiting()? {
            break if status.success() {
                (Ok(()), false)
            } else {
                (
                    Err(
                        format!("全栈命令失败，退出状态 {status}；核对阶段收据，不回显子进程内容")
                            .into(),
                    ),
                    true,
                )
            };
        }
        thread::sleep(Duration::from_millis(20));
    };
    child.close();
    drop(child);
    let mut bytes = output.join().map_err(|_| "全栈输出线程失败")??;
    let errors = errors.join().map_err(|_| "全栈错误输出线程失败")??;
    if let Err(error) = result {
        let reason = crate::process::exit_status::command_output_diagnostic(&bytes, &errors);
        if output_mode == CaptureOutput::ApiControl
            && failed_exit
            && !overflow.load(Ordering::Acquire)
            && let Some(diagnostic) = worker::api_control_diagnostic(&bytes)
        {
            return Err(format!("{error}{reason}；{diagnostic}").into());
        }
        return Err(format!("{error}{reason}").into());
    }
    if output_mode == CaptureOutput::Merged {
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

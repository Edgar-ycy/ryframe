use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
};

const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) enum StdoutTarget {
    Capture,
    File(PathBuf),
    HeldFile { path: PathBuf, file: File },
}

pub(crate) struct CommandSpec {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) stdout: StdoutTarget,
    pub(crate) work_dir: PathBuf,
    pub(crate) label: String,
    pub(crate) timeout: Duration,
}

pub(crate) struct CommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl CommandOutput {
    pub(crate) fn success(mut self, label: &str) -> Result<Self> {
        if self.status.success() {
            return Ok(self);
        }
        let detail = redact(&String::from_utf8_lossy(&self.stderr), &BTreeMap::new());
        self.stderr = detail.as_bytes().to_vec();
        Err(format!(
            "{label} 失败（{}）：{}",
            self.status,
            if detail.trim().is_empty() {
                "子进程未返回 stderr"
            } else {
                detail.trim()
            }
        )
        .into())
    }

    pub(crate) fn utf8(&self, label: &str) -> Result<String> {
        String::from_utf8(self.stdout.clone()).map_err(|_| format!("{label} 输出不是 UTF-8").into())
    }
}

pub(crate) trait Transport {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput>;
    /// 私有 SQL 只保留退出状态。模拟传输可以沿用同一受控 run。
    fn run_private(&self, spec: CommandSpec) -> Result<CommandOutput> {
        if cfg!(test) {
            return self.run(spec);
        }
        Err("当前传输未提供私有 stdin 执行".into())
    }
    fn s3(&self, request: super::super::s3::Request) -> Result<serde_json::Value>;
    /// 来源缓存写端口必须由受管实现显式接通；普通测试 transport 不得误连真实 Redis。
    fn source_cache_cleanup(&self, _arguments: &serde_json::Value) -> Result<serde_json::Value> {
        Err("当前资源端口未授权来源缓存清理；请使用正式受管实现，禁止隐式连接或重试".into())
    }
    /// 本地证据只允许独占新建、完整写入并同步；不覆盖已有现场。
    fn publish_evidence(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }
}

pub(crate) struct SystemTransport;

impl Transport for SystemTransport {
    fn source_cache_cleanup(&self, arguments: &serde_json::Value) -> Result<serde_json::Value> {
        crate::recovery::source::evidence::cleanup_source_cache(arguments)
    }
    fn s3(&self, request: super::super::s3::Request) -> Result<serde_json::Value> {
        super::super::s3::execute(request)
    }
    fn run_private(&self, spec: CommandSpec) -> Result<CommandOutput> {
        if !matches!(spec.stdout, StdoutTarget::Capture) {
            return Err("私有命令只保留退出状态".into());
        }
        let input = spec.stdin.as_deref().ok_or("私有命令缺少 stdin")?;
        // Windows 使用独占的关闭即删除句柄，名称只在句柄生存期内存在。
        let mut file = tempfile::tempfile_in(&spec.work_dir)?;
        file.write_all(input)?;
        file.flush()?;
        file.rewind()?;
        let mut command = prepare_command(&spec)?;
        command
            .stdin(std::process::Stdio::from(file))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let status = spawn_and_wait(command, &spec, None)?;
        Ok(CommandOutput {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let mut command = prepare_command(&spec)?;
        fs::create_dir_all(&spec.work_dir).map_err(|error| {
            format!(
                "无法创建 {} 的临时证据目录 {}：{error}",
                spec.label,
                spec.work_dir.display()
            )
        })?;
        let token = Uuid::new_v4().simple().to_string();
        let captured_stdout = matches!(spec.stdout, StdoutTarget::Capture)
            .then(|| spec.work_dir.join(format!(".{token}.stdout")));
        let stdout_path = match &spec.stdout {
            StdoutTarget::Capture => captured_stdout
                .as_ref()
                .expect("Capture 具有输出路径")
                .clone(),
            StdoutTarget::File(path) | StdoutTarget::HeldFile { path, .. } => path.clone(),
        };
        let stderr_path = spec.work_dir.join(format!(".{token}.stderr"));
        let stdin_path = spec.work_dir.join(format!(".{token}.stdin"));
        let stdout = match &spec.stdout {
            StdoutTarget::HeldFile { file, .. } => file
                .try_clone()
                .map_err(|error| format!("无法复制已持有的 stdout 句柄：{error}"))?,
            StdoutTarget::Capture | StdoutTarget::File(_) => create_file(&stdout_path, "stdout")?,
        };
        let stderr = create_file(&stderr_path, "stderr")?;
        let stdin = create_stdin(&stdin_path, spec.stdin.as_deref())?;

        command
            .stdin(match stdin {
                Some(file) => std::process::Stdio::from(file),
                None => std::process::Stdio::null(),
            })
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr));
        let status = spawn_and_wait(command, &spec, Some(&stderr_path))?;
        let stdout_bytes = if captured_stdout.is_some() {
            read_limited(&stdout_path, &spec.label)?
        } else {
            Vec::new()
        };
        let stderr_bytes = read_limited(&stderr_path, &spec.label)?;
        let secret_values = secret_values(&spec.environment);
        let stderr_bytes =
            redact(&String::from_utf8_lossy(&stderr_bytes), &secret_values).into_bytes();
        if let Some(path) = captured_stdout {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(stderr_path);
        let _ = fs::remove_file(stdin_path);
        Ok(CommandOutput {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    }
}

fn prepare_command(spec: &CommandSpec) -> Result<Command> {
    let mut command = child_command(&spec.program);
    configure_environment(&mut command, &spec.environment, std::env::vars_os())?;
    command.args(&spec.args).current_dir(&spec.cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    Ok(command)
}

fn spawn_and_wait(
    command: Command,
    spec: &CommandSpec,
    stderr: Option<&Path>,
) -> Result<ExitStatus> {
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command).map_err(|error| {
        format!(
            "无法启动 {}（{}）：{error}",
            spec.label,
            spec.program.display()
        )
    })?;
    wait_status(&mut child, spec, stderr)
}

/// 显式字段保持优先；仅 Windows 系统 DLL/Socket 所需的 SystemRoot 可来自 OS。
pub(crate) fn configure_environment(
    command: &mut Command,
    environment: &BTreeMap<String, String>,
    operating_system: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<()> {
    command.env_clear().envs(environment);
    #[cfg(windows)]
    {
        const MISSING_ROOT: &str =
            "Windows 子进程缺少非空 OS SystemRoot；请修复控制器的 Windows 系统环境后再执行显式操作";
        if let Some((_, value)) = environment
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("SystemRoot"))
        {
            if value.is_empty() {
                return Err(MISSING_ROOT.into());
            }
        } else {
            let value = operating_system
                .into_iter()
                .find(|(name, _)| name.as_encoded_bytes().eq_ignore_ascii_case(b"SystemRoot"))
                .map(|(_, value)| value)
                .filter(|value| !value.is_empty())
                .ok_or(MISSING_ROOT)?;
            command.env("SystemRoot", value);
        }
    }
    #[cfg(not(windows))]
    let _ = operating_system;
    Ok(())
}

fn wait_status(
    child: &mut ManagedChild,
    spec: &CommandSpec,
    stderr: Option<&Path>,
) -> Result<ExitStatus> {
    let deadline = Instant::now() + spec.timeout;
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            None => {
                let stopped = stop_child(child);
                if let Some(stderr) = stderr {
                    return Err(format!(
                        "{} 超过 {} 秒，已回收完整子进程树；诊断保留在 {}",
                        spec.label,
                        spec.timeout.as_secs(),
                        stderr.display()
                    )
                    .into());
                }
                let state = if stopped.is_ok() {
                    "已回收子进程树"
                } else {
                    "子进程树回收未证实"
                };
                return Err(
                    format!("{} 超过 {} 秒，{state}", spec.label, spec.timeout.as_secs()).into(),
                );
            }
        }
    }
}

fn create_stdin(path: &Path, input: Option<&[u8]>) -> Result<Option<File>> {
    let Some(input) = input else {
        return Ok(None);
    };
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法创建 stdin 证据 {}：{error}", path.display()))?;
    file.write_all(input)?;
    file.flush()?;
    file.rewind()?;
    Ok(Some(file))
}

fn create_file(path: &Path, label: &str) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法创建 {label} 证据 {}：{error}", path.display()).into())
}

fn read_limited(path: &Path, label: &str) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_CAPTURE_BYTES {
        return Err(format!(
            "{label} 输出超过 {} MiB，已保留证据 {}",
            MAX_CAPTURE_BYTES / (1024 * 1024),
            path.display()
        )
        .into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn secret_values(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    environment
        .iter()
        .filter(|(key, value)| {
            !value.is_empty()
                && (key.contains("KEY")
                    || key.contains("PASSWORD")
                    || key.contains("PASSWD")
                    || key.contains("SECRET")
                    || key.contains("TOKEN")
                    || key.contains("CREDENTIAL")
                    || key.contains("MYSQL_PWD"))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub(crate) fn redact(text: &str, secrets: &BTreeMap<String, String>) -> String {
    let mut result = text.to_owned();
    let mut values = secrets.values().collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for value in values {
        result = result.replace(value, "[REDACTED]");
    }
    result
}

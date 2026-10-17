use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::{
    Result,
    process::{ChildGroup, child_command, stop_child},
};

const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) enum StdoutTarget {
    Capture,
    File(PathBuf),
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
}

pub(crate) struct SystemTransport;

impl Transport for SystemTransport {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
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
        let stdout_path = captured_stdout
            .as_ref()
            .cloned()
            .or_else(|| match &spec.stdout {
                StdoutTarget::File(path) => Some(path.clone()),
                StdoutTarget::Capture => None,
            })
            .expect("stdout 目标必须存在");
        let stderr_path = spec.work_dir.join(format!(".{token}.stderr"));
        let stdin_path = spec.work_dir.join(format!(".{token}.stdin"));
        let stdout = create_file(&stdout_path, "stdout")?;
        let stderr = create_file(&stderr_path, "stderr")?;
        let stdin = if let Some(input) = spec.stdin.as_deref() {
            let mut file = create_file(&stdin_path, "stdin")?;
            file.write_all(input)?;
            file.flush()?;
            Some(file)
        } else {
            None
        };

        let mut command = child_command(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.environment)
            .stdin(match stdin {
                Some(file) => std::process::Stdio::from(file),
                None => std::process::Stdio::null(),
            })
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let group = ChildGroup::new()?;
        let mut child = group.spawn(command).map_err(|error| {
            format!(
                "无法启动 {}（{}）：{error}",
                spec.label,
                spec.program.display()
            )
        })?;
        let deadline = Instant::now() + spec.timeout;
        let status = loop {
            match child.try_wait()? {
                Some(status) => break status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                None => {
                    let _ = stop_child(&mut child);
                    return Err(format!(
                        "{} 超过 {} 秒，已回收完整子进程树；诊断保留在 {}",
                        spec.label,
                        spec.timeout.as_secs(),
                        stderr_path.display()
                    )
                    .into());
                }
            }
        };
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

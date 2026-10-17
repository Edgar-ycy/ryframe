use std::{
    env,
    fs::OpenOptions,
    io::Read,
    path::Path,
    process::{Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::Value;

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
};

pub(crate) trait Api {
    fn get(&self, endpoint: &str, timeout: Duration) -> Result<Value>;
    fn download(&self, endpoint: &str, destination: &Path, timeout: Duration) -> Result<()>;
}

pub(crate) struct GhApi;

impl Api for GhApi {
    fn get(&self, endpoint: &str, timeout: Duration) -> Result<Value> {
        let mut command = gh_command(endpoint);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_capture(command, timeout, endpoint)?;
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("GitHub API 响应不是有效 JSON：{error}").into())
    }

    fn download(&self, endpoint: &str, destination: &Path, timeout: Duration) -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|error| format!("无法新建证据下载文件 {}：{error}", destination.display()))?;
        let mut command = gh_command(endpoint);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::piped());
        run_status(command, timeout, endpoint)?;
        Ok(())
    }
}

fn gh_command(endpoint: &str) -> Command {
    let mut command = child_command("gh");
    command.args(["api", endpoint]);
    command
}

fn run_capture(command: Command, timeout: Duration, endpoint: &str) -> Result<Output> {
    let mut child = ChildGroup::new()?.spawn(command)?;
    let stdout = spawn_reader(child.take_stdout_reader()?, "stdout");
    let stderr = spawn_reader(child.take_stderr_reader()?, "stderr");
    let status = wait_for_child(&mut child, timeout, endpoint)?;
    drop(child);
    let stdout = join_reader(stdout, "stdout")?;
    let stderr = join_reader(stderr, "stderr")?;
    if !status.success() {
        return Err(api_status_error(endpoint, status, &stderr));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn run_status(command: Command, timeout: Duration, endpoint: &str) -> Result<()> {
    let mut child = ChildGroup::new()?.spawn(command)?;
    let stderr = spawn_reader(child.take_stderr_reader()?, "stderr");
    let status = wait_for_child(&mut child, timeout, endpoint)?;
    drop(child);
    let stderr = join_reader(stderr, "stderr")?;
    if !status.success() {
        return Err(api_status_error(endpoint, status, &stderr));
    }
    Ok(())
}

fn wait_for_child(
    child: &mut ManagedChild,
    timeout: Duration,
    endpoint: &str,
) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            stop_child(child)?;
            return Ok(status);
        }
        if Instant::now() >= deadline {
            stop_child(child)?;
            return Err(format!(
                "GitHub API 请求超时：{}（上限 {:.1} 秒）",
                sanitized_endpoint(endpoint),
                timeout.as_secs_f64()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_reader(
    reader: Option<Box<dyn Read + Send>>,
    label: &'static str,
) -> Option<thread::JoinHandle<std::result::Result<Vec<u8>, String>>> {
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|error| format!("读取 gh {label} 失败：{error}"))?;
            Ok(bytes)
        })
    })
}

fn join_reader(
    reader: Option<thread::JoinHandle<std::result::Result<Vec<u8>, String>>>,
    label: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| format!("读取 gh {label} 的线程发生 panic"))?
        .map_err(Into::into)
}

fn api_status_error(
    endpoint: &str,
    status: ExitStatus,
    stderr: &[u8],
) -> Box<dyn std::error::Error> {
    let detail = redact_diagnostic(&String::from_utf8_lossy(stderr));
    let detail = detail.trim();
    if detail.is_empty() {
        format!(
            "GitHub API 请求失败：{}（退出状态 {status}）",
            sanitized_endpoint(endpoint)
        )
        .into()
    } else {
        format!(
            "GitHub API 请求失败：{}（退出状态 {status}）：{detail}",
            sanitized_endpoint(endpoint)
        )
        .into()
    }
}

fn sanitized_endpoint(endpoint: &str) -> String {
    endpoint
        .split(['?', '#'])
        .next()
        .unwrap_or(endpoint)
        .to_owned()
}

pub(crate) fn redact_diagnostic(value: &str) -> String {
    let mut redacted = value.to_owned();
    if let Ok(token) = env::var("GH_TOKEN")
        && !token.is_empty()
    {
        redacted = redacted.replace(&token, "[REDACTED]");
    }
    let authorization =
        Regex::new(r"(?i)(authorization\s*:\s*(?:bearer\s+)?)\S+").expect("固定脱敏正则有效");
    redacted = authorization
        .replace_all(&redacted, "${1}[REDACTED]")
        .into_owned();
    let token =
        Regex::new(r"(?i)((?:token|secret|password)\s*[=:]\s*)\S+").expect("固定脱敏正则有效");
    redacted = token.replace_all(&redacted, "${1}[REDACTED]").into_owned();
    let url_credentials = Regex::new(r"(https?://)[^/@\s]+@").expect("固定脱敏正则有效");
    redacted = url_credentials
        .replace_all(&redacted, "${1}[REDACTED]@")
        .into_owned();
    redacted
        .split_whitespace()
        .map(|part| {
            if part.starts_with("ghp_")
                || part.starts_with("gho_")
                || part.starts_with("ghu_")
                || part.starts_with("ghs_")
                || part.starts_with("ghr_")
                || part.starts_with("github_pat_")
            {
                "[REDACTED]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

//! 长期监督器：同 ELF 早期 exec、自有 Child 和精确 pidfd 共同持有。
use super::super::super::{digest, json as doc, resources::common};
use super::super::diagnostic::{self, Category, Outcome, failure};
use super::{Launch, inputs, owned};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    os::fd::OwnedFd,
    path::Path,
    process::{Child, Stdio},
    time::{Duration, Instant},
};
use tokio::signal::unix::{Signal, SignalKind, signal};

struct Signals {
    term: Signal,
    hup: Signal,
    interrupt: Signal,
}
impl Signals {
    fn new() -> Result<Self> {
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }
    async fn pause(&mut self) -> Result<()> {
        tokio::select! {
            _ = self.term.recv() => Err(failure(Category::SupervisorInterrupted, Outcome::Unknown)),
            _ = self.hup.recv() => Err(failure(Category::SupervisorInterrupted, Outcome::Unknown)),
            _ = self.interrupt.recv() => Err(failure(Category::SupervisorInterrupted, Outcome::Unknown)),
            _ = tokio::time::sleep(Duration::from_millis(50)) => Ok(()),
        }
    }
}

struct Started {
    child: Child,
    fd: Option<OwnedFd>,
    expected: Option<Value>,
    returncode: Option<i32>,
}
impl Started {
    fn cleanup(&mut self) -> Result<()> {
        let fd = self
            .fd
            .as_ref()
            .ok_or("缓存子进程未取得 pidfd，清理未知；禁止裸PID回退")?;
        owned::stop(fd)?;
        if !owned::exited(fd, Duration::from_secs(10))? {
            return Err("缓存清理退出未确认".into());
        }
        self.returncode = Some(exit_code(self.child.wait()?)?);
        Ok(())
    }
}

pub(super) async fn serve(launch: &Launch, request_path: &Path, sha: &str) -> Result<()> {
    inputs::validate(launch)?;
    let request = &launch.request;
    if !request["previous_identity"].is_null()
        && request["previous_boot_id"] == inputs::boot()?
        && inputs::identity(u32::try_from(
            request["previous_identity"]["pid"]
                .as_u64()
                .ok_or("前代PID无效")?,
        )?)?
        .is_some()
    {
        return Err("同 boot 原 Redis PID 仍存在或已复用".into());
    }
    inputs::bind_and_close(inputs::port(launch)?)?;
    let mut signals = Signals::new()?;
    let config = super::super::model::linux_path(doc::text(
        &request["configuration"]["path"],
        "Redis 配置",
    )?)?;
    inputs::receipt(
        launch,
        "linux-intent.json",
        json!({"request_sha256":launch.request_sha256,
        "arguments":[request["launcher"],config],"supervisor":inputs::identity(std::process::id())?}),
    )?;
    let mut started = None;
    let result = run(launch, request_path, sha, &mut signals, &mut started).await;
    if let Err(error) = &result {
        let cleanup = started.as_mut().map(Started::cleanup).transpose();
        let completed = cleanup.is_ok();
        let child = started.as_ref();
        let mut detail = diagnostic::Diagnostic::decode(&diagnostic::from_error(error.as_ref()))?;
        detail.write_outcome = if completed {
            Outcome::Stopped
        } else {
            Outcome::Unknown
        };
        inputs::receipt(
            launch,
            "linux-failure.json",
            json!({
                "error_type":"NativeCacheFailure",
                "diagnostic":detail.value(),
                "cleanup_error_type":if completed {Value::Null} else {json!("NativeCleanupUnknown")},
                "identity":child.and_then(|c|c.expected.as_ref()),
                "request_sha256":launch.request_sha256,"cleanup_completed":completed,
                "child_pid":child.map(|c|c.child.id()),"child_returncode":child.and_then(|c|c.returncode),
                "popen_called":child.is_some(),
                "intent":common::binding(&launch.linux_output.join("linux-intent.json"))?
            }),
        )?;
        return Err(Box::new(detail));
    }
    result
}

async fn run(
    launch: &Launch,
    request_path: &Path,
    sha: &str,
    signals: &mut Signals,
    started: &mut Option<Started>,
) -> Result<()> {
    spawn(launch, request_path, sha, started)?;
    let child = started.as_mut().ok_or("缓存自有子进程缺失")?;
    let expected = wait_for_exec(launch, child, signals).await?;
    child.expected = Some(expected.clone());
    inputs::receipt(
        launch,
        "linux-process.json",
        json!({"request_sha256":launch.request_sha256,
        "identity":expected,"intent":common::binding(&launch.linux_output.join("linux-intent.json"))?}),
    )?;
    accept(launch, child, signals, &expected).await?;
    let fd = child.fd.as_ref().ok_or("缓存未取得 pidfd")?;
    while !owned::exited(fd, Duration::ZERO)? {
        signals.pause().await?;
    }
    child.returncode = Some(exit_code(child.child.wait()?)?);
    inputs::receipt(
        launch,
        "linux-exited.json",
        json!({"identity":expected,"returncode":child.returncode}),
    )?;
    launch.native_tool.verify_linux()?;
    if digest::file(request_path)?.sha256 != sha {
        return Err("缓存监督结束时冻结请求变化".into());
    }
    Ok(())
}

fn spawn(
    launch: &Launch,
    request_path: &Path,
    sha: &str,
    started: &mut Option<Started>,
) -> Result<()> {
    let output = &launch.linux_output;
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("redis.stdout.log"))?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("redis.stderr.log"))?;
    let directory = super::super::model::linux_path(doc::text(
        &launch.request["directory"]["path"],
        "Redis 目录",
    )?)?;
    let child = std::process::Command::new(&launch.native_tool.linux_program)
        .arg("__linux-cache-exec")
        .arg("--request")
        .arg(request_path)
        .arg("--sha256")
        .arg(sha)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    *started = Some(Started {
        child,
        fd: None,
        expected: None,
        returncode: None,
    });
    let owned = started.as_mut().ok_or("缓存自有子进程缺失")?;
    owned.fd = Some(owned::open_child(&owned.child)?);
    Ok(())
}

async fn wait_for_exec(launch: &Launch, child: &Started, signals: &mut Signals) -> Result<Value> {
    let fd = child.fd.as_ref().ok_or("缓存未取得 pidfd")?;
    let timeout = launch.request["timeout_seconds"]
        .as_u64()
        .ok_or("缓存超时无效")?;
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let boot = inputs::boot()?;
    let mut first_start = None;
    loop {
        if owned::exited(fd, Duration::ZERO)? {
            let file = std::fs::File::open(launch.linux_output.join("redis.stdout.log"))?;
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(16385).read_to_end(&mut bytes)?;
            return Err(match diagnostic::Diagnostic::decode_output(&bytes) {
                Ok(detail) => Box::new(detail),
                Err(_) => failure(Category::ExecFailed, Outcome::Unknown),
            });
        }
        if Instant::now() >= deadline {
            return Err(failure(Category::ExecWaitTimeout, Outcome::Unknown));
        }
        if let Some(identity) = inputs::identity(child.child.id())? {
            let start = doc::text(&identity["started"], "缓存创建时间")?.to_owned();
            if first_start.as_ref().is_some_and(|first| first != &start)
                || identity["boot_id"] != boot
            {
                return Err("缓存 exec 窗口创建身份变化".into());
            }
            first_start = Some(start);
            if identity["executable"] == launch.request["executable"] {
                inputs::validate(launch)?;
                if inputs::identity(child.child.id())?.as_ref() != Some(&identity)
                    || owned::exited(fd, Duration::ZERO)?
                {
                    return Err("Redis 映像核对期间身份变化".into());
                }
                return Ok(identity);
            }
            if identity["executable"] != launch.native_tool.linux_program {
                return Err("缓存 exec 出现未知映像".into());
            }
        }
        signals.pause().await?;
    }
}

async fn accept(
    launch: &Launch,
    child: &Started,
    signals: &mut Signals,
    expected: &Value,
) -> Result<()> {
    let fd = child.fd.as_ref().ok_or("缓存未取得 pidfd")?;
    let timeout = launch.request["timeout_seconds"]
        .as_u64()
        .ok_or("缓存超时无效")?;
    let deadline = Instant::now() + Duration::from_secs(timeout + 15);
    let output = &launch.linux_output;
    while !output.join("acknowledged.json").try_exists()? {
        if owned::exited(fd, Duration::ZERO)? || Instant::now() >= deadline {
            return Err(failure(Category::AcknowledgementTimeout, Outcome::Unknown));
        }
        signals.pause().await?;
    }
    let ack = doc::read(&output.join("acknowledged.json"))?;
    if ack != json!({"request_sha256":launch.request_sha256,"identity":expected}) {
        return Err("缓存确认不属于本次创建身份".into());
    }
    inputs::validate(launch)?;
    if owned::exited(fd, Duration::ZERO)?
        || inputs::identity(child.child.id())?.as_ref() != Some(expected)
    {
        return Err("缓存接受确认前已退出或身份变化".into());
    }
    inputs::receipt(launch, "accepted.json", ack)
}

fn exit_code(status: std::process::ExitStatus) -> Result<i32> {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .ok_or_else(|| "缓存子进程退出码或信号未知".into())
}

#[cfg(test)]
#[test]
fn receipts_preserve_normal_and_negative_signal_exit_codes() {
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        exit_code(std::process::ExitStatus::from_raw(7 << 8)).unwrap(),
        7
    );
    assert_eq!(
        exit_code(std::process::ExitStatus::from_raw(15)).unwrap(),
        -15
    );
    assert_eq!(
        exit_code(std::process::ExitStatus::from_raw(9)).unwrap(),
        -9
    );
}

use super::failure::{Failure, NativeResult};
use serde_json::Value;
use std::{
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct CancellationState {
    requested: AtomicBool,
    parent: Option<Cancellation>,
}
#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<CancellationState>);
impl Cancellation {
    pub(crate) fn linked(&self) -> Self {
        Self(Arc::new(CancellationState {
            requested: AtomicBool::new(false),
            parent: Some(self.clone()),
        }))
    }
    pub(crate) fn cancel(&self) {
        self.0.requested.store(true, Ordering::Release);
    }
    pub(crate) fn check(&self) -> NativeResult<()> {
        if self.0.requested.load(Ordering::Acquire)
            || self
                .0
                .parent
                .as_ref()
                .is_some_and(|parent| parent.check().is_err())
        {
            Err(Failure::new("request_cancelled"))
        } else {
            Ok(())
        }
    }
    pub(crate) async fn during<T>(
        &self,
        future: impl Future<Output = NativeResult<T>>,
    ) -> NativeResult<T> {
        self.check()?;
        let cancelled = async {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if self.check().is_err() {
                    break;
                }
            }
        };
        futures_util::pin_mut!(future, cancelled);
        match futures_util::future::select(future, cancelled).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right(_) => Err(Failure::new("request_cancelled")),
        }
    }
    pub(crate) async fn sleep(&self, duration: Duration) -> NativeResult<()> {
        self.during(async {
            tokio::time::sleep(duration).await;
            Ok(())
        })
        .await
    }
}

/// 有界 stdin/stdout 协议。进程树归属由既有 ChildGroup 管理，输出不写日志。
pub(crate) async fn json(
    executable: &Path,
    arguments: &[String],
    cwd: &Path,
    input: &Value,
    timeout: Duration,
    limit: usize,
    cancel: &Cancellation,
) -> NativeResult<Value> {
    cancel.check()?;
    let mut stdin = tempfile::tempfile().map_err(|_| Failure::new("process_input"))?;
    let bytes = serde_json::to_vec(input).map_err(|_| Failure::new("process_input"))?;
    stdin
        .write_all(&bytes)
        .and_then(|()| stdin.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|_| Failure::new("process_input"))?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // 不把项目配置或凭据继续传入外部工具。
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if super::entry::private_environment(&name)
            || matches!(name.as_str(), "PYTHONUTF8" | "PYTHONIOENCODING")
        {
            command.env_remove(key);
        }
    }
    let group = crate::process::ChildGroup::new().map_err(|_| Failure::new("process_spawn"))?;
    let mut child = group
        .spawn(command)
        .map_err(|_| Failure::new("process_spawn"))?;
    let mut stdout = child
        .take_stdout_reader()
        .map_err(|_| Failure::new("process_output"))?
        .ok_or_else(|| Failure::new("process_output"))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let flagged = overflow.clone();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes);
        if bytes.len() > limit {
            flagged.store(true, Ordering::Release);
        }
        read.map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        if cancel.check().is_err() {
            break Err(Failure::new("request_cancelled"));
        }
        if overflow.load(Ordering::Acquire) {
            break Err(Failure::new("process_output_limit"));
        }
        if started.elapsed() >= timeout {
            break Err(Failure::new("process_timeout"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(_) => break Err(Failure::new("process_wait")),
        }
    };
    drop(child);
    let output = reader
        .join()
        .map_err(|_| Failure::new("process_output"))?
        .map_err(|_| Failure::new("process_output"))?;
    let status = status?;
    if output.len() > limit {
        return Err(Failure::new("process_output_limit"));
    }
    if !status.success() {
        return Err(Failure::new("process_exit"));
    }
    let text = std::str::from_utf8(&output).map_err(|_| Failure::new("process_output"))?;
    crate::recovery::clone::native::json::parse_fragment(text, "原生测量协议")
        .map_err(|_| Failure::new("process_response"))
}

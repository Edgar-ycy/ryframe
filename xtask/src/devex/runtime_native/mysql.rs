//! 只读任务观察的文件凭据连接，复用原有 SQL/终态模型。
use super::process::Cancellation;
use crate::devex::job_evidence::{
    mysql::{
        invocation,
        io::{self, Line, Response},
        options::OptionFile,
    },
    protocol::{EvidenceResult, Session},
};
use serde_json::Value;
use std::{process::Command, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) struct Mysql {
    runtime: tokio::runtime::Runtime,
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: BufReader<tokio::process::ChildStdout>,
    _options: CredentialFile,
    cancel: Cancellation,
    timeout: Duration,
    failed: bool,
    #[cfg(windows)]
    group: Option<processkit::ProcessGroup>,
    #[cfg(unix)]
    group: u32,
}

enum CredentialFile {
    Managed(OptionFile),
    Temporary(tempfile::NamedTempFile),
}

impl CredentialFile {
    fn verify(&self) -> EvidenceResult<()> {
        match self {
            Self::Managed(options) => options.verify(),
            Self::Temporary(options) => options
                .as_file()
                .metadata()
                .map(|_| ())
                .map_err(|_| "mysql-options"),
        }
    }
}

impl Mysql {
    pub(crate) fn open(
        binding: &Value,
        cancel: Cancellation,
        timeout: Duration,
    ) -> EvidenceResult<Self> {
        let (command, options) = command(binding)?;
        Self::open_with_credential(
            command,
            CredentialFile::Managed(options),
            &[],
            cancel,
            timeout,
        )
    }

    pub(crate) fn open_command(
        command: Command,
        options: tempfile::NamedTempFile,
        prefix: &[u8],
        cancel: Cancellation,
        timeout: Duration,
    ) -> EvidenceResult<Self> {
        Self::open_with_credential(
            command,
            CredentialFile::Temporary(options),
            prefix,
            cancel,
            timeout,
        )
    }

    fn open_with_credential(
        command: Command,
        options: CredentialFile,
        prefix: &[u8],
        cancel: Cancellation,
        timeout: Duration,
    ) -> EvidenceResult<Self> {
        options.verify()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "mysql-runtime")?;
        let _entered = runtime.enter();
        #[cfg(windows)]
        let (mut child, group) = {
            let group = processkit::ProcessGroup::new().map_err(|_| "mysql-job")?;
            let child = group
                .spawn(tokio::process::Command::from(command))
                .map_err(|_| "mysql-spawn")?;
            (child, Some(group))
        };
        #[cfg(not(windows))]
        let mut child = {
            let mut command = tokio::process::Command::from(command);
            #[cfg(unix)]
            command.process_group(0);
            command.spawn().map_err(|_| "mysql-spawn")?
        };
        #[cfg(unix)]
        let group = child.id().ok_or("mysql-spawn")?;
        let input = child.stdin.take().ok_or("mysql-pipes")?;
        let output = BufReader::new(child.stdout.take().ok_or("mysql-pipes")?);
        drop(_entered);
        let mut session = Self {
            runtime,
            child,
            input: Some(input),
            output,
            _options: options,
            cancel,
            timeout,
            failed: false,
            #[cfg(any(windows, unix))]
            group,
        };
        // 先由 RAII 接管完整进程树，再写容器凭据前缀；管道早退也必须回收。
        if !prefix.is_empty() {
            session.runtime.block_on(async {
                let input = session.input.as_mut().ok_or("mysql-pipes")?;
                tokio::time::timeout(
                    timeout,
                    session.cancel.during(async {
                        input
                            .write_all(prefix)
                            .await
                            .map_err(|_| super::failure::Failure::new("mysql-stdin-write"))?;
                        input
                            .flush()
                            .await
                            .map_err(|_| super::failure::Failure::new("mysql-stdin-write"))
                    }),
                )
                .await
                .map_err(|_| "mysql-init-timeout")?
                .map_err(|error| error.category)
            })?;
        }
        Ok(session)
    }
    fn execute(&mut self, statement: &str) -> EvidenceResult<Vec<Value>> {
        if self.failed {
            return Err("mysql-closed");
        }
        if self
            .child
            .try_wait()
            .map_err(|_| "mysql-process-status")?
            .is_some_and(|status| !status.success())
        {
            return Err("mysql-process-exit");
        }
        let (bytes, sentinel) = io::query(statement)?;
        let future = async {
            self.input
                .as_mut()
                .ok_or("mysql-closed")?
                .write_all(&bytes)
                .await
                .map_err(|_| "mysql-stdin-write")?;
            self.input
                .as_mut()
                .ok_or("mysql-closed")?
                .flush()
                .await
                .map_err(|_| "mysql-stdin-write")?;
            let mut response = Response::new(sentinel);
            loop {
                let line = line(&mut self.output).await?;
                if response.accept(line)? {
                    return Ok(response.finish());
                }
            }
        };
        let result = self
            .runtime
            .block_on(self.cancel.during(async {
                tokio::time::timeout(self.timeout, future)
                    .await
                    .map_err(|_| super::failure::Failure::new("mysql-timeout"))?
                    .map_err(super::failure::Failure::new)
            }))
            .map_err(|error| error.category);
        if result.is_err() {
            self.failed = true;
            self.input.take();
            let _ = self.child.start_kill();
        }
        result
    }
}
impl Session for Mysql {
    fn query(&mut self, statement: &str) -> EvidenceResult<Vec<Value>> {
        self.execute(statement)
    }
    fn close(&mut self) -> EvidenceResult<()> {
        self.execute("ROLLBACK")?;
        self.input.take();
        self.runtime.block_on(async {
            let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .map_err(|_| "mysql-close-timeout")?
                .map_err(|_| "mysql-wait")?;
            if !status.success() {
                return Err("mysql-close-exit");
            }
            Ok(())
        })
    }
}
impl Drop for Mysql {
    fn drop(&mut self) {
        self.input.take();
        #[cfg(windows)]
        drop(self.group.take());
        #[cfg(unix)]
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.group as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = self.child.start_kill();
        let _ = self.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await
        });
    }
}

pub(crate) fn command(binding: &Value) -> EvidenceResult<(Command, OptionFile)> {
    let environment = std::env::vars_os().collect();
    let invocation = invocation::command(binding, &environment)?;
    Ok((invocation.command, invocation.options))
}

async fn line(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
) -> EvidenceResult<Line> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().await.map_err(|_| "mysql-stdout-read")?;
        if chunk.is_empty() {
            return Err("mysql-eof");
        }
        let length = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(chunk.len(), |index| index + 1);
        if bytes.len() + length > io::MAX_RESPONSE_BYTES {
            return Err("mysql-output-limit");
        }
        let complete = chunk[length - 1] == b'\n';
        bytes.extend_from_slice(&chunk[..length]);
        reader.consume(length);
        if complete {
            break;
        }
    }
    let wire_bytes = bytes.len();
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Line { bytes, wire_bytes })
}

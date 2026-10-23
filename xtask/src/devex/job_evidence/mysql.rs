use std::{
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value;

#[path = "mysql/invocation.rs"]
pub(crate) mod invocation;
#[path = "mysql/io.rs"]
pub(crate) mod io;
#[path = "mysql/options.rs"]
pub(crate) mod options;

use io::{ReadEvent, Response, WriteRequest};

const POLL: Duration = Duration::from_millis(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// 独占一个持久 mysql CLI 连接；任意查询失败后关闭，禁止重连或重放。
pub(crate) struct MysqlSession {
    child: Option<Child>,
    options: Option<options::OptionFile>,
    writes: Option<SyncSender<WriteRequest>>,
    lines: Option<Receiver<ReadEvent>>,
    readers: Vec<JoinHandle<()>>,
    stderr_failure: Arc<AtomicU8>,
    failed: bool,
}

impl MysqlSession {
    /// binding 与采集器协议一致，包含明文 control.password 和绝对 mysql_client。
    /// SQL、schema、scope 和 server_uuid 的业务所有权由父层在同一会话内核验。
    pub(crate) fn open(binding: &Value) -> Result<Self, &'static str> {
        let environment = std::env::vars_os().collect();
        Self::open_with_environment(binding, &environment)
    }

    /// 使用显式环境创建会话，与默认入口共用全部校验，不修改父进程环境。
    pub(crate) fn open_with_environment(
        binding: &Value,
        environment: &invocation::Environment,
    ) -> Result<Self, &'static str> {
        let (child, options) = invocation::command(binding, environment)?.spawn()?;
        let mut session = Self {
            child: Some(child),
            options: Some(options),
            writes: None,
            lines: None,
            readers: Vec::new(),
            stderr_failure: Arc::new(AtomicU8::new(0)),
            failed: false,
        };
        if let Err(reason) = session.start_threads() {
            session.terminate()?;
            return Err(reason);
        }
        Ok(session)
    }

    fn start_threads(&mut self) -> Result<(), &'static str> {
        let child = self.child.as_mut().ok_or("mysql-closed")?;
        let stdin = child.stdin.take().ok_or("mysql-pipes")?;
        let stdout = child.stdout.take().ok_or("mysql-pipes")?;
        let stderr = child.stderr.take().ok_or("mysql-pipes")?;
        let (events, lines) = mpsc::sync_channel(2);
        let (writes, requests) = mpsc::sync_channel(1);
        self.lines = Some(lines);
        self.writes = Some(writes);
        let reader = thread::Builder::new()
            .name("mysql-stdout".into())
            .spawn(move || io::read_stdout(stdout, events))
            .map_err(|_| "mysql-reader-start")?;
        self.readers.push(reader);
        let failure = Arc::clone(&self.stderr_failure);
        let reader = thread::Builder::new()
            .name("mysql-stderr".into())
            .spawn(move || io::drain_stderr(stderr, failure))
            .map_err(|_| "mysql-reader-start")?;
        self.readers.push(reader);
        let writer = thread::Builder::new()
            .name("mysql-stdin".into())
            .spawn(move || io::write_stdin(stdin, requests))
            .map_err(|_| "mysql-writer-start")?;
        self.readers.push(writer);
        Ok(())
    }

    pub(crate) fn execute(
        &mut self,
        sql: &str,
        timeout: Duration,
    ) -> Result<Vec<Value>, &'static str> {
        if self.failed || self.child.is_none() {
            return Err("mysql-closed");
        }
        let result = self.execute_inner(sql, timeout);
        if result.is_err() {
            self.failed = true;
            self.terminate()?;
        }
        result
    }

    fn execute_inner(&mut self, sql: &str, timeout: Duration) -> Result<Vec<Value>, &'static str> {
        let deadline = Instant::now().checked_add(timeout).ok_or("mysql-timeout")?;
        self.check_running()?;
        let (bytes, sentinel) = io::query(sql)?;
        let (complete, written) = mpsc::sync_channel(1);
        self.writes
            .as_ref()
            .ok_or("mysql-closed")?
            .try_send(WriteRequest { bytes, complete })
            .map_err(|_| "mysql-stdin-write")?;
        let mut response = Response::new(sentinel);
        let mut write_complete = false;
        loop {
            let wait = remaining(deadline)?;
            self.check_running()?;
            // 写入完成前也排空输出，避免两端管道与有界通道互相等待。
            if !write_complete {
                match written.try_recv() {
                    Ok(result) => {
                        result?;
                        write_complete = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => return Err("mysql-stdin-write"),
                }
            }
            let event = self
                .lines
                .as_ref()
                .ok_or("mysql-closed")?
                .recv_timeout(wait);
            match event {
                Ok(line) => {
                    if response.accept(line?)? {
                        if !write_complete {
                            self.wait_written(&written, deadline)?;
                        }
                        remaining(deadline)?;
                        self.check_running()?;
                        return Ok(response.finish());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("mysql-reader-stopped"),
            }
        }
    }

    fn wait_written(
        &mut self,
        written: &Receiver<Result<(), &'static str>>,
        deadline: Instant,
    ) -> Result<(), &'static str> {
        loop {
            let wait = remaining(deadline)?;
            self.check_running()?;
            match written.recv_timeout(wait) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("mysql-stdin-write"),
            }
        }
    }

    fn check_running(&mut self) -> Result<(), &'static str> {
        let result = match self.child.as_mut().ok_or("mysql-closed")?.try_wait() {
            Ok(None) => Ok(()),
            // 正常退出仍需读取已缓冲的完整响应，EOF 本身不能代替 sentinel。
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(_)) => Err("mysql-process-exit"),
            Err(_) => Err("mysql-process-status"),
        };
        process_result(result, self.stderr_failure.load(Ordering::Acquire))
    }

    /// 健康连接先在同一连接内 ROLLBACK，再关闭 stdin 并等待正常退出。
    pub(crate) fn close(&mut self) -> Result<(), &'static str> {
        if self.child.is_none() {
            self.clean_options()?;
            return if self.failed {
                Err("mysql-closed")
            } else {
                Ok(())
            };
        }
        let result = self.execute("ROLLBACK", CLOSE_TIMEOUT).map(|_| ());
        if let Err(error) = result {
            self.terminate()?;
            return Err(error);
        }
        // 写线程收到断开后退出并释放其独占的 stdin。
        self.writes.take();
        let result = self.wait_exit(CLOSE_TIMEOUT);
        let cleanup = self.terminate();
        let result = process_result(
            cleanup.and(result),
            self.stderr_failure.load(Ordering::Acquire),
        );
        self.failed |= result.is_err();
        result
    }

    fn wait_exit(&mut self, timeout: Duration) -> Result<(), &'static str> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("mysql-close-timeout")?;
        loop {
            match self.child.as_mut().ok_or("mysql-closed")?.try_wait() {
                Ok(Some(status)) if status.success() => {
                    self.child.take();
                    return Ok(());
                }
                Ok(Some(_)) => return Err("mysql-close-exit"),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
                Ok(None) => return Err("mysql-close-timeout"),
                Err(_) => return Err("mysql-process-status"),
            }
        }
    }

    fn terminate(&mut self) -> Result<(), &'static str> {
        // 先断开有界通道，避免 reader 卡在 send，或 writer 等待下一条请求。
        self.lines.take();
        self.writes.take();
        let mut result = Ok(());
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    if child.kill().is_err() {
                        result = Err("mysql-kill");
                    }
                }
            }
            if child.wait().is_err() {
                self.child = Some(child);
                return Err("mysql-wait");
            }
        }
        for reader in self.readers.drain(..) {
            if reader.join().is_err() {
                result = Err("mysql-reader-panic");
            }
        }
        self.clean_options()?;
        result
    }

    fn clean_options(&mut self) -> Result<(), &'static str> {
        if let Some(options) = &mut self.options {
            options.close()?;
        }
        self.options = None;
        Ok(())
    }
}

impl Drop for MysqlSession {
    fn drop(&mut self) {
        // Drop 不执行 SQL、不 unwrap，也不传播线程 panic。
        if let Err(reason) = self.terminate() {
            if self.child.is_some()
                && let Some(options) = &mut self.options
            {
                options.preserve();
            }
            eprintln!("{reason}");
        }
    }
}

/// stderr 字节可能只是合法警告；仅读取失败或已有进程错误使结果失败。
pub(crate) fn process_result(
    result: Result<(), &'static str>,
    stderr_failure: u8,
) -> Result<(), &'static str> {
    result?;
    if stderr_failure == 2 {
        Err("mysql-stderr-read")
    } else {
        Ok(())
    }
}

fn remaining(deadline: Instant) -> Result<Duration, &'static str> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .map(|duration| duration.min(POLL))
        .ok_or("mysql-timeout")
}

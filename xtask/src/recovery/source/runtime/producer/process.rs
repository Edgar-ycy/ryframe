//! 受管树内的有界 ready/stdin 握手；取消、超限、写授权失败均不重试。
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{Inputs, launch};
use crate::{
    Result,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
    recovery::clone::native::{json as document, target_verify::guard},
};

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/runtime/producer/process.rs"]
mod tests;

enum Event {
    Ready(std::io::Result<Vec<u8>>),
    Stdout(std::io::Result<Vec<u8>>),
    Stderr(std::io::Result<Vec<u8>>),
}

struct Readers {
    receiver: Receiver<Event>,
    threads: Vec<JoinHandle<()>>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

pub(super) fn run(
    inputs: &Inputs<'_>,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Value> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_controlled(inputs, checkpoint))
}

async fn run_controlled(
    inputs: &Inputs<'_>,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Value> {
    checkpoint()?;
    let operation = uuid::Uuid::new_v4().simple().to_string();
    let intent = launch::prepare(inputs, &operation)?;
    let tool = guard::Inputs::open([inputs.tool.to_owned()])?;
    let argv = document::array(&intent["argv"], "来源生产者实际命令")?
        .iter()
        .map(|arg| document::text(arg, "来源生产者参数").map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let mut command = child_command(inputs.tool);
    command
        .args(argv.iter().skip(1))
        .current_dir(inputs.backend)
        .env_clear()
        .envs(inputs.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGroup::new()?.spawn(command)?;
    let readers = match Readers::start(&mut child) {
        Ok(readers) => readers,
        Err(error) => {
            return match finish_child(child) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!(
                    "来源生产者管道建立失败：{error}；受管树回收未确认：{cleanup}；保留现场，禁止重放"
                ).into()),
            };
        }
    };
    let result = execute(inputs, &intent, &tool, &mut child, &readers, checkpoint).await;
    let cleanup = finish_child(child);
    let joined = readers.finish(inputs.directory, result.is_err());
    match (result, cleanup, joined) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (Err(error), Ok(()), Ok(())) => Err(error),
        (result, cleanup, joined) => Err(format!(
            "来源生产者未确认完成，禁止重放；原错误：{}；回收诊断：{}；管道诊断：{}",
            result
                .err()
                .map_or_else(|| "无".into(), |error| error.to_string()),
            cleanup
                .err()
                .map_or_else(|| "无".into(), |error| error.to_string()),
            joined
                .err()
                .map_or_else(|| "无".into(), |error| error.to_string())
        )
        .into()),
    }
}

async fn execute(
    inputs: &Inputs<'_>,
    intent: &Value,
    tool: &guard::Inputs,
    child: &mut ManagedChild,
    readers: &Readers,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Value> {
    let producer = launch::register(inputs, intent, child.id())?;
    let mut stdin = child
        .take_stdin_writer()?
        .ok_or("来源生产者缺少 stdin 管道")?;
    let controlled = async {
        let raw = readers.ready(child).await?;
        let ready =
            launch::write_bytes(&inputs.directory.join("source-producer-ready.json"), &raw)?;
        inputs.ready(intent)?;
        checkpoint()?;
        inputs.source_unchanged()?;
        inputs.staging(intent)?;
        inputs.registered()?;
        launch::unchanged(&producer)?;
        tool.check()?;
        let mut authorization = serde_json::to_vec(
            &json!({"operation":"start","run_dir":inputs.directory,
            "operation_id":intent["operation_id"],"source_generation_sha256":inputs.start["sha256"]}),
        )?;
        authorization.push(b'\n');
        stdin.write_all(&authorization)?;
        stdin.flush()?;
        drop(stdin);
        let (status, stdout, stderr) = readers.output(child).await?;
        launch::complete(
            inputs,
            &producer,
            &ready,
            &stdout,
            &stderr,
            status.code().ok_or("来源生产者被信号终止，未确认完成")?,
        )?;
        inputs.source_unchanged()?;
        inputs.staging(intent)?;
        tool.check()?;
        inputs.verify()
    };
    tokio::pin!(controlled);
    tokio::select! {
        result = &mut controlled => result,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|_| "无法建立来源生产者取消观察")?;
            Err("来源业务验证已取消，结果可能未知；保留 intent 和进程登记，禁止重放".into())
        }
    }
}

impl Readers {
    fn start(child: &mut ManagedChild) -> Result<Self> {
        let stdout = child
            .take_stdout_reader()?
            .ok_or("来源生产者缺少 stdout 管道")?;
        let stderr = child
            .take_stderr_reader()?
            .ok_or("来源生产者缺少 stderr 管道")?;
        let (sender, receiver) = mpsc::channel();
        let output = sender.clone();
        let captured_stdout = Arc::new(Mutex::new(Vec::new()));
        let captured_stderr = Arc::new(Mutex::new(Vec::new()));
        let captured = captured_stdout.clone();
        let first = thread::spawn(move || read_stdout(stdout, output, &captured));
        let captured = captured_stderr.clone();
        let second = thread::spawn(move || {
            let result = limited(stderr, 1024 * 1024, &captured);
            let _ = sender.send(Event::Stderr(result));
        });
        Ok(Self {
            receiver,
            threads: vec![first, second],
            stdout: captured_stdout,
            stderr: captured_stderr,
        })
    }

    async fn ready(&self, child: &mut ManagedChild) -> Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match self.receiver.try_recv() {
                Ok(Event::Ready(Ok(raw)))
                    if !raw.is_empty() && raw.len() <= 64 * 1024 && raw.ends_with(b"\n") =>
                {
                    return Ok(raw);
                }
                Ok(_) => return Err("来源生产者在有效 ready 前退出或管道读取失败".into()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("来源生产者 ready 管道中断".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                return Err("来源生产者未在 60 秒授权窗口内发布有效 ready".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn output(&self, child: &mut ManagedChild) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        let deadline = Instant::now() + Duration::from_secs(1800);
        let (mut status, mut stdout, mut stderr) = (None, None, None);
        loop {
            while let Ok(event) = self.receiver.try_recv() {
                match event {
                    Event::Stdout(value) if stdout.is_none() => {
                        stdout = Some(value.map_err(|_| "来源生产者 stdout 超限或读取失败")?)
                    }
                    Event::Stderr(value) if stderr.is_none() => {
                        stderr = Some(value.map_err(|_| "来源生产者 stderr 超限或读取失败")?)
                    }
                    _ => return Err("来源生产者出现重复或越序管道事件".into()),
                }
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if status.is_some() && stdout.is_some() && stderr.is_some() {
                return Ok((
                    status.ok_or("生产者退出状态缺失")?,
                    stdout.ok_or("stdout 缺失")?,
                    stderr.ok_or("stderr 缺失")?,
                ));
            }
            if Instant::now() >= deadline {
                return Err("来源生产者超过 1800 秒，业务结果未知；保留证据，禁止重放".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn finish(self, directory: &std::path::Path, failed: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.threads.iter().any(|thread| !thread.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let mut pending = false;
        let mut panicked = false;
        for thread in self.threads {
            if thread.is_finished() {
                panicked |= thread.join().is_err();
            } else {
                // 未完成线程仍持有自己的管道；只分离句柄，不能无界 join。
                pending = true;
            }
        }
        let publication = if failed || pending || panicked {
            preserve_partial(directory, &self.stdout, &self.stderr)
        } else {
            Ok(())
        };
        if pending || panicked {
            return Err(format!(
                "来源生产者管道未在有界回收窗口内关闭或读取线程异常；禁止重放；部分输出保存结果：{}",
                publication.err().map_or_else(|| "已保存可取得的输出".into(), |error| error.to_string())
            ).into());
        }
        publication
    }
}

fn read_stdout(stdout: Box<dyn Read + Send>, sender: Sender<Event>, captured: &Mutex<Vec<u8>>) {
    let mut reader = BufReader::new(stdout);
    let mut raw = Vec::new();
    let ready = Read::take(&mut reader, 64 * 1024 + 1)
        .read_until(b'\n', &mut raw)
        .map(|_| raw);
    let valid = ready
        .as_ref()
        .is_ok_and(|raw| !raw.is_empty() && raw.len() <= 64 * 1024 && raw.ends_with(b"\n"));
    let _ = sender.send(Event::Ready(ready));
    if valid {
        let _ = sender.send(Event::Stdout(limited(reader, 16 * 1024 * 1024, captured)));
    }
}

fn limited(
    mut reader: impl Read,
    max: usize,
    captured: &Mutex<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = max.saturating_sub(bytes.len());
        let allowed = count.min(remaining);
        bytes.extend_from_slice(&chunk[..allowed]);
        captured
            .lock()
            .map_err(|_| std::io::ErrorKind::Other)?
            .extend_from_slice(&chunk[..allowed]);
        if allowed != count {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
    Ok(bytes)
}

fn finish_child(mut child: ManagedChild) -> Result<()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = stop_child(&mut child).map_err(|error| error.to_string());
        // 即使 Drop 等待失败，所有权也留在回收线程中，不使父流程无限等待。
        drop(child);
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("来源生产者受管树回收失败：{error}").into()),
        Err(_) => {
            Err("来源生产者受管树未在 5 秒内确认回收；原登记身份和回收线程仍保留，禁止重放".into())
        }
    }
}

fn preserve_partial(
    directory: &std::path::Path,
    stdout: &Mutex<Vec<u8>>,
    stderr: &Mutex<Vec<u8>>,
) -> Result<()> {
    // 原协议只有 ready 之后才允许发布输出；没有 ready 时不伪造成功前缀。
    if !directory.join("source-producer-ready.json").try_exists()? {
        return Ok(());
    }
    for (name, captured) in [
        ("source-producer.stdout.json", stdout),
        ("source-producer.stderr.log", stderr),
    ] {
        let path = directory.join(name);
        if path.try_exists()? {
            continue;
        }
        let bytes = captured
            .try_lock()
            .map_err(|_| "来源部分输出当前不可取得；保留原进程和证据，禁止重放")?;
        launch::write_bytes(&path, &bytes)?;
    }
    Ok(())
}

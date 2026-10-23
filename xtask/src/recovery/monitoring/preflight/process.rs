use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Result,
    process::{ChildGroup, stop_child},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;

/// stdin 使用运行目录内受管文件；超时、失败和超限输出都先回收整个进程树。
pub(crate) fn capture(
    run: &Path,
    mut command: Command,
    input: &[u8],
    timeout: Duration,
) -> Result<Output> {
    let mut protocol = tempfile::tempfile_in(run)?;
    protocol.write_all(input)?;
    protocol.seek(SeekFrom::Start(0))?;
    command
        .stdin(Stdio::from(protocol))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    let reader = child.take_stdout_reader()?.ok_or("监控预检缺少输出管道")?;
    let output = thread::Builder::new()
        .name("monitoring-preflight".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            reader
                .take((OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("监控预检期限无效")?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => break Err("监控预检超时"),
            Err(_) => break Err("无法核验监控预检进程状态"),
        }
    };
    let cleanup = stop_child(&mut child);
    let bytes = output.join().map_err(|_| "监控预检输出线程失败")?;
    cleanup?;
    let status = status?;
    let stdout = bytes.map_err(|_| "无法读取监控预检输出")?;
    if stdout.len() > OUTPUT_LIMIT {
        return Err("监控预检输出超过 1 MiB".into());
    }
    Ok(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

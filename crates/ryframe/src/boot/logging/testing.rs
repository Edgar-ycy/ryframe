use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tracing_subscriber::layer::SubscriberExt;

use super::{LoggerLevel, configured_filter};

const INFO_SENTINEL: &str = "configured-filter-info-sentinel";
const ERROR_SENTINEL: &str = "configured-filter-error-sentinel";
type Reader = JoinHandle<io::Result<Vec<u8>>>;

#[test]
fn error_case_emits_only_error() {
    emit_case(LoggerLevel::Error, false);
}

#[test]
fn trace_case_emits_info_and_error() {
    emit_case(LoggerLevel::Trace, true);
}

fn emit_case(level: LoggerLevel, expects_info: bool) {
    let subscriber = tracing_subscriber::registry()
        .with(configured_filter(level))
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_target(false)
                .with_writer(std::io::stdout),
        );
    // 线程局部 subscriber 让这两个固定 case 也能在完整测试中独立执行。
    tracing::subscriber::with_default(subscriber, || {
        assert_eq!(tracing::enabled!(tracing::Level::INFO), expects_info);
        assert!(tracing::enabled!(tracing::Level::ERROR));
        tracing::info!("{INFO_SENTINEL}");
        tracing::error!("{ERROR_SENTINEL}");
    });
}

#[test]
fn configured_error_ignores_rust_log_trace() {
    assert_child_output(
        "boot::logging::testing::error_case_emits_only_error",
        "trace",
        false,
    );
}

#[test]
fn configured_trace_ignores_rust_log_off() {
    assert_child_output(
        "boot::logging::testing::trace_case_emits_info_and_error",
        "off",
        true,
    );
}

fn assert_child_output(test: &str, rejected_override: &str, expects_info: bool) {
    let output = run_case(test, rejected_override)
        .unwrap_or_else(|error| panic!("日志过滤子进程失败：{error}"));
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "case={test}；状态={}；输出={text}",
        output.status
    );
    assert!(
        text.contains("1 passed"),
        "case={test} 未精确执行一个测试；输出={text}"
    );
    assert!(
        text.contains(&format!("ERROR {ERROR_SENTINEL}")),
        "case={test} 缺少 ERROR；输出={text}"
    );
    assert_eq!(
        text.contains(INFO_SENTINEL),
        expects_info,
        "case={test} 的 INFO 不符合文件级别；输出={text}"
    );
    if expects_info {
        assert!(
            text.contains(&format!("INFO {INFO_SENTINEL}")),
            "case={test} 缺少实际 INFO 日志；输出={text}"
        );
    }
}

fn run_case(test: &str, rejected_override: &str) -> Result<Output, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("定位当前 lib 测试二进制：{error}"))?;
    let mut command = Command::new(executable);
    command
        .args(["--exact", test, "--nocapture"])
        .env("RUST_LOG", rejected_override)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let child = command
        .spawn()
        .map_err(|error| format!("启动固定 case {test}：{error}"))?;
    let mut process = CaseProcess {
        child,
        readers: Vec::new(),
        reaped: false,
    };
    let stdout = process.child.stdout.take().ok_or("缺少 stdout 管道")?;
    process.readers.push(start_reader(stdout, "stdout")?);
    let stderr = process.child.stderr.take().ok_or("缺少 stderr 管道")?;
    process.readers.push(start_reader(stderr, "stderr")?);
    process.finish(deadline)
}

fn start_reader(mut stream: impl Read + Send + 'static, name: &str) -> Result<Reader, String> {
    thread::Builder::new()
        .name(format!("logging-filter-{name}"))
        .spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map_err(|error| format!("启动 {name} 收集线程：{error}"))
}

struct CaseProcess {
    child: Child,
    readers: Vec<Reader>,
    reaped: bool,
}

impl CaseProcess {
    fn finish(&mut self, deadline: Instant) -> Result<Output, String> {
        let status = self.wait_until(deadline);
        let mut errors = Vec::new();
        if let Err(error) = &status {
            errors.push(error.clone());
        }
        if !self.reaped {
            errors.extend(self.stop());
        }
        let streams = self.collect(&mut errors);
        if !errors.is_empty() {
            let captured = streams
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!("{}；输出={captured}", errors.join("；")));
        }
        let mut streams = streams.into_iter();
        Ok(Output {
            status: status?,
            stdout: streams.next().ok_or("stdout 收集结果缺失")?,
            stderr: streams.next().ok_or("stderr 收集结果缺失")?,
        })
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<ExitStatus, String> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    return Ok(status);
                }
                Ok(None) => {}
                Err(error) => return Err(format!("查询固定 case 退出状态：{error}")),
            }
            if Instant::now() >= deadline {
                return Err("固定 case 超过10秒，精确终止并回收持有的子进程".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        // 始终按持有的 Child 回收，不根据名称或测试名终止其他进程。
        if let Err(error) = self.child.kill() {
            errors.push(format!("终止固定 case：{error}"));
        }
        match self.child.wait() {
            Ok(_) => self.reaped = true,
            Err(error) => errors.push(format!("回收固定 case：{error}")),
        }
        errors
    }

    fn collect(&mut self, errors: &mut Vec<String>) -> Vec<Vec<u8>> {
        let mut streams = Vec::new();
        for (index, reader) in self.readers.drain(..).enumerate() {
            match reader.join() {
                Ok(Ok(bytes)) => streams.push(bytes),
                Ok(Err(error)) => errors.push(format!("读取管道{index}：{error}")),
                Err(_) => errors.push(format!("管道{index}读取线程异常退出")),
            }
        }
        streams
    }
}

impl Drop for CaseProcess {
    fn drop(&mut self) {
        let mut errors = if self.reaped { Vec::new() } else { self.stop() };
        drop(self.collect(&mut errors));
        if !errors.is_empty() {
            eprintln!("日志过滤子进程清理失败：{}", errors.join("；"));
        }
    }
}

//! 一次观察调用的 launcher 所有权；不推断或控制 Linux 后代。
use crate::Result;
use serde_json::Value;
use std::{
    fmt, io,
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cause {
    Metadata(io::ErrorKind),
    Poll(io::ErrorKind),
    Timeout,
    OutputLimit,
    Read(io::ErrorKind),
    Exit(Option<i32>),
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kill {
    AlreadyExited,
    Requested,
    Failed(io::ErrorKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cleanup {
    kill: Kill,
    wait: std::result::Result<Option<i32>, io::ErrorKind>,
}

#[derive(Debug)]
pub(crate) struct Failure {
    cause: Cause,
    cleanup: Cleanup,
}

impl Failure {
    pub(crate) fn is_timeout(&self) -> bool {
        self.cause == Cause::Timeout
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (category, reason) = match self.cause {
            Cause::Timeout => ("LinuxObserverTimeout", "观察调用超过20秒"),
            Cause::OutputLimit => ("LinuxObserverOutputLimit", "观察输出超过4MiB"),
            Cause::Metadata(_) => ("LinuxObserverMetadata", "无法读取观察输出元数据"),
            Cause::Poll(_) => ("LinuxObserverPoll", "无法查询launcher退出状态"),
            Cause::Read(_) => ("LinuxObserverRead", "无法读取观察响应"),
            Cause::Exit(_) => ("LinuxObserverExit", "观察进程非零退出"),
            Cause::Protocol => ("LinuxObserverProtocol", "观察响应无效"),
        };
        let detail = match self.cause {
            Cause::Metadata(kind) | Cause::Poll(kind) | Cause::Read(kind) => format!("{kind:?}"),
            Cause::Exit(code) => format!("exit={code:?}"),
            _ => "无附加私有输出".to_owned(),
        };
        write!(output, "{category}：{reason}；主因详情={detail}；launcher ")?;
        match self.cleanup.kill {
            Kill::AlreadyExited => output.write_str("已观察退出，未发送kill")?,
            Kill::Requested => output.write_str("kill已请求")?,
            Kill::Failed(kind) => write!(output, "kill失败({kind:?})")?,
        }
        match self.cleanup.wait {
            Ok(code) => write!(output, "，wait已确认launcher退出(code={code:?})")?,
            Err(kind) => write!(output, "，wait失败({kind:?})，launcher退出未确认")?,
        }
        output.write_str("；Linux/Git后代状态未知；保留现场，禁止自动重放")
    }
}
impl std::error::Error for Failure {}

// 只抽取当前 Child 的三个操作，供同一错误路径确定性核验，不开放进程号或信号。
trait Launcher {
    fn poll(&mut self) -> io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self, remaining: Duration) -> io::Result<ExitStatus>;
}
impl Launcher for Child {
    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait()
    }
    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self, remaining: Duration) -> io::Result<ExitStatus> {
        let started = Instant::now();
        poll_until_exit(
            remaining,
            || self.try_wait(),
            || started.elapsed(),
            std::thread::sleep,
        )?;
        // 只有try_wait已确认退出，才调用不会再等待活进程的标准wait取结果。
        Child::wait(self)
    }
}

fn poll_until_exit(
    remaining: Duration,
    mut poll: impl FnMut() -> io::Result<Option<ExitStatus>>,
    mut elapsed: impl FnMut() -> Duration,
    mut pause: impl FnMut(Duration),
) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = poll()? {
            return Ok(status);
        }
        let left = remaining.saturating_sub(elapsed());
        if left.is_zero() {
            // 预算耗尽也做最终非阻塞核对，不等待、不发送其他信号。
            return poll()?.ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut));
        }
        pause(left.min(Duration::from_millis(20)));
    }
}

pub(super) fn run(
    child: &mut Child,
    length: impl FnMut() -> io::Result<u64>,
    read: impl FnOnce(ExitStatus) -> std::result::Result<Value, Cause>,
) -> Result<Value> {
    let started = Instant::now();
    run_with(
        child,
        length,
        read,
        || started.elapsed(),
        || std::thread::sleep(Duration::from_millis(20)),
    )
}

fn run_with(
    child: &mut impl Launcher,
    mut length: impl FnMut() -> io::Result<u64>,
    read: impl FnOnce(ExitStatus) -> std::result::Result<Value, Cause>,
    mut elapsed: impl FnMut() -> Duration,
    mut pause: impl FnMut(),
) -> Result<Value> {
    let mut exited = false;
    let result = (|| {
        let status = loop {
            if length().map_err(|error| Cause::Metadata(error.kind()))? > super::super::LIMIT as u64
            {
                return Err(Cause::OutputLimit);
            }
            if elapsed() > Duration::from_secs(20) {
                return Err(Cause::Timeout);
            }
            if let Some(status) = child.poll().map_err(|error| Cause::Poll(error.kind()))? {
                exited = true;
                break status;
            }
            pause();
        };
        read(status)
    })();
    result.map_err(|cause| {
        let kill = if exited {
            Kill::AlreadyExited
        } else {
            match child.kill() {
                Ok(()) => Kill::Requested,
                Err(error) => Kill::Failed(error.kind()),
            }
        };
        // kill失败仍须有界wait；剩余预算来自原调用时钟，不能重新计20秒。
        let remaining = Duration::from_secs(20).saturating_sub(elapsed());
        let wait = child
            .wait(remaining)
            .map(|status| status.code())
            .map_err(|error| error.kind());
        Box::new(Failure {
            cause,
            cleanup: Cleanup { kill, wait },
        }) as Box<dyn std::error::Error>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Owned {
        events: Vec<&'static str>,
        poll: io::Result<Option<ExitStatus>>,
        kill: io::Result<()>,
        wait: io::Result<ExitStatus>,
        remaining: Vec<Duration>,
    }
    fn status() -> ExitStatus {
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(0)
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(0)
        }
    }
    impl Owned {
        fn new() -> Self {
            Self {
                events: vec![],
                poll: Ok(None),
                kill: Ok(()),
                wait: Ok(status()),
                remaining: vec![],
            }
        }
    }
    impl Launcher for Owned {
        fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
            self.events.push("poll");
            std::mem::replace(&mut self.poll, Ok(None))
        }
        fn kill(&mut self) -> io::Result<()> {
            self.events.push("kill");
            std::mem::replace(&mut self.kill, Ok(()))
        }
        fn wait(&mut self, remaining: Duration) -> io::Result<ExitStatus> {
            self.events.push("wait");
            self.remaining.push(remaining);
            std::mem::replace(&mut self.wait, Ok(status()))
        }
    }

    #[test]
    fn metadata_poll_timeout_and_output_limit_always_cleanup_owned_launcher() {
        for cause in [
            Cause::Metadata(io::ErrorKind::PermissionDenied),
            Cause::Poll(io::ErrorKind::Other),
            Cause::Timeout,
            Cause::OutputLimit,
        ] {
            let mut child = Owned::new();
            if matches!(cause, Cause::Poll(_)) {
                child.poll = Err(io::Error::from(io::ErrorKind::Other));
            }
            let error = run_with(
                &mut child,
                || match cause {
                    Cause::Metadata(kind) => Err(io::Error::from(kind)),
                    Cause::OutputLimit => Ok(super::super::super::LIMIT as u64 + 1),
                    _ => Ok(0),
                },
                |_| panic!("失败后不能读取响应"),
                || {
                    if cause == Cause::Timeout {
                        Duration::from_secs(21)
                    } else {
                        Duration::ZERO
                    }
                },
                || {},
            )
            .unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.cause, cause);
            assert_eq!(failure.is_timeout(), cause == Cause::Timeout);
            assert!(child.events.ends_with(&["kill", "wait"]));
            assert_eq!(failure.cleanup.wait, Ok(Some(0)));
            assert_eq!(
                child.remaining,
                [if cause == Cause::Timeout {
                    Duration::ZERO
                } else {
                    Duration::from_secs(20)
                }]
            );
            assert!(failure.to_string().contains("Linux/Git后代状态未知"));
        }
    }

    #[test]
    fn failed_kill_still_waits_and_cleanup_cannot_replace_timeout_cause() {
        for wait_fails in [false, true] {
            let mut child = Owned::new();
            child.kill = Err(io::Error::from(io::ErrorKind::PermissionDenied));
            if wait_fails {
                child.wait = Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            let error = run_with(
                &mut child,
                || Ok(0),
                |_| unreachable!(),
                || Duration::from_secs(21),
                || {},
            )
            .unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert!(failure.is_timeout());
            assert_eq!(child.events, ["kill", "wait"]);
            assert_eq!(child.remaining, [Duration::ZERO]);
            assert_eq!(
                failure.cleanup.kill,
                Kill::Failed(io::ErrorKind::PermissionDenied)
            );
            assert_eq!(failure.cleanup.wait.is_err(), wait_fails);
            let message = failure.to_string();
            assert!(message.contains("LinuxObserverTimeout") && message.contains("kill失败"));
            assert_eq!(message.contains("launcher退出未确认"), wait_fails);
        }
    }

    #[test]
    fn cleanup_uses_remaining_deadline_and_expiry_only_permits_nonblocking_checks() {
        use std::cell::Cell;
        let mut child = Owned::new();
        child.kill = Err(io::Error::from(io::ErrorKind::PermissionDenied));
        child.wait = Err(io::Error::from(io::ErrorKind::TimedOut));
        let error = run_with(
            &mut child,
            || Err(io::Error::from(io::ErrorKind::Other)),
            |_| unreachable!(),
            || Duration::from_secs(7),
            || {},
        )
        .unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.cause, Cause::Metadata(io::ErrorKind::Other));
        assert!(!failure.is_timeout());
        assert_eq!(child.remaining, [Duration::from_secs(13)]);
        assert_eq!(child.events, ["kill", "wait"]);
        assert!(failure.to_string().contains("launcher退出未确认"));

        let elapsed = Cell::new(Duration::ZERO);
        let polls = Cell::new(0);
        let error = poll_until_exit(
            Duration::from_millis(13),
            || {
                polls.set(polls.get() + 1);
                Ok(None)
            },
            || elapsed.get(),
            |duration| {
                assert_eq!(duration, Duration::from_millis(13));
                elapsed.set(elapsed.get() + duration);
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(elapsed.get(), Duration::from_millis(13));
        assert_eq!(polls.get(), 3);
        for exited_on_final_check in [false, true] {
            let polls = Cell::new(0);
            let result = poll_until_exit(
                Duration::ZERO,
                || {
                    polls.set(polls.get() + 1);
                    Ok((exited_on_final_check && polls.get() == 2).then(status))
                },
                || Duration::ZERO,
                |_| panic!("耗尽预算不能sleep"),
            );
            assert_eq!(polls.get(), 2);
            assert_eq!(result.is_ok(), exited_on_final_check);
            if let Err(error) = result {
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            }
        }
    }

    #[test]
    fn exited_launcher_response_failures_wait_without_killing_or_retrying() {
        for cause in [
            Cause::Read(io::ErrorKind::Other),
            Cause::Protocol,
            Cause::Exit(Some(2)),
        ] {
            let mut child = Owned::new();
            child.poll = Ok(Some(status()));
            let error = run_with(
                &mut child,
                || Ok(0),
                |_| Err(cause),
                || Duration::ZERO,
                || {},
            )
            .unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.cause, cause);
            assert!(!failure.is_timeout());
            assert_eq!(child.events, ["poll", "wait"]);
        }
        let mut child = Owned::new();
        child.poll = Ok(Some(status()));
        assert_eq!(
            run_with(
                &mut child,
                || Ok(0),
                |_| Ok(Value::Null),
                || Duration::ZERO,
                || {}
            )
            .unwrap(),
            Value::Null
        );
        assert_eq!(child.events, ["poll"]);
    }
}

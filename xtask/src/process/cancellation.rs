use std::{
    cell::RefCell,
    io::Read,
    process::{Command, ExitStatus, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::Result;

use super::{ChildGroup, ManagedChild, stop_child};

thread_local! {
    static PROCESS_CANCELLATION: RefCell<Option<ProcessCancellation>> = const { RefCell::new(None) };
}

#[derive(Clone, Default)]
pub(crate) struct ProcessCancellation {
    requested: Arc<AtomicBool>,
}

impl ProcessCancellation {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct ProcessCancelled {
    cleanup_error: Option<String>,
}

impl std::fmt::Display for ProcessCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("并行任务已因兄弟任务失败而取消")?;
        if let Some(error) = &self.cleanup_error {
            write!(formatter, "；进程树回收报告：{error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProcessCancelled {}

struct ProcessCancellationGuard(Option<ProcessCancellation>);

impl Drop for ProcessCancellationGuard {
    fn drop(&mut self) {
        PROCESS_CANCELLATION.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

pub(crate) fn with_process_cancellation<T>(
    cancellation: &ProcessCancellation,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let previous = PROCESS_CANCELLATION.with(|slot| slot.replace(Some(cancellation.clone())));
    let _guard = ProcessCancellationGuard(previous);
    action()
}

pub(crate) fn is_process_cancellation(error: &(dyn std::error::Error + 'static)) -> bool {
    error.downcast_ref::<ProcessCancelled>().is_some()
}

pub(super) fn command_status(mut command: Command) -> Result<ExitStatus> {
    let Some(cancellation) = current_process_cancellation() else {
        return Ok(command.status()?);
    };
    reject_cancelled_before_spawn(&cancellation)?;
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    wait_for_command(&mut child, &cancellation)
}

pub(super) fn command_output_result(mut command: Command) -> Result<Output> {
    let Some(cancellation) = current_process_cancellation() else {
        return Ok(command.output()?);
    };
    reject_cancelled_before_spawn(&cancellation)?;
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    let stdout = spawn_output_reader(child.take_stdout_reader()?);
    let stderr = spawn_output_reader(child.take_stderr_reader()?);
    let status = wait_for_command(&mut child, &cancellation);
    // 直接父进程退出后仍可能有后代持有输出管道；先释放命令级进程组，保证读取线程
    // 不会等待不再属于本次命令合同的孤儿进程。
    drop(child);
    let stdout = join_output_reader(stdout, "stdout");
    let stderr = join_output_reader(stderr, "stderr");
    Ok(Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    })
}

fn reject_cancelled_before_spawn(cancellation: &ProcessCancellation) -> Result<()> {
    if cancellation.requested() {
        return Err(ProcessCancelled {
            cleanup_error: None,
        }
        .into());
    }
    Ok(())
}

fn current_process_cancellation() -> Option<ProcessCancellation> {
    PROCESS_CANCELLATION.with(|slot| slot.borrow().clone())
}

fn wait_for_command(
    child: &mut ManagedChild,
    cancellation: &ProcessCancellation,
) -> Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancellation.requested() {
            let cleanup_error = stop_child(child).err().map(|error| error.to_string());
            return Err(ProcessCancelled { cleanup_error }.into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_output_reader(
    reader: Option<Box<dyn Read + Send>>,
) -> Option<thread::JoinHandle<std::io::Result<Vec<u8>>>> {
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut output = Vec::new();
            reader.read_to_end(&mut output)?;
            Ok(output)
        })
    })
}

fn join_output_reader(
    reader: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    label: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| format!("读取命令 {label} 的线程发生 panic"))?
        .map_err(Into::into)
}

use std::{error::Error, fmt, path::Path, time::Duration};

#[cfg(not(windows))]
use std::process::{Command, Stdio};

#[cfg(windows)]
use processkit::{Command, Outcome, ProcessGroup, Stdin, StdioMode};

use serde_json::json;

use crate::{
    Result,
    process::failure_exit_code,
    recovery::{dataset_prepare::MARKER, monitoring::execution::ProtectedInputs},
};
#[cfg(windows)]
use crate::{
    process::close_owned_windows_job, recovery::runtime::start::actor::system_environment,
};
#[cfg(not(windows))]
use crate::{
    process::{command_output_result_with_timeout, protocol_stdin},
    recovery::runtime::start::actor::restrict_environment,
};

#[derive(Debug)]
pub(super) struct ChildFailure {
    pub(super) status: Option<i32>,
    pub(super) confirmed_closed: bool,
    reason: &'static str,
    executable_check_failed: bool,
}

impl ChildFailure {
    fn managed(error: &(dyn Error + 'static)) -> Self {
        Self::unconfirmed(
            failure_exit_code(error),
            "参考数据准备子进程启动、等待或整树回收未获确认",
        )
    }

    fn unconfirmed(status: Option<i32>, reason: &'static str) -> Self {
        Self {
            status,
            confirmed_closed: false,
            reason,
            executable_check_failed: false,
        }
    }
}

impl fmt::Display for ChildFailure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.reason)?;
        if let Some(status) = self.status {
            write!(output, "（退出码 {status}）")?;
        }
        if self.executable_check_failed {
            output.write_str("；当前协调器可执行文件后验失败")?;
        }
        output.write_str("；保留原阶段收据，禁止自动重放")
    }
}

impl Error for ChildFailure {}

pub(super) fn run(backend: &Path, plan: &Path, preflight: &Path, timeout: Duration) -> Result<()> {
    if !(Duration::from_secs(1)..=Duration::from_secs(604_800)).contains(&timeout) {
        return Err("参考数据准备阶段时限必须在 1 至 604800 秒之间".into());
    }
    let executable = std::env::current_exe().map_err(|_| "无法确定当前参考恢复协调器可执行文件")?;
    let command = command(&executable, backend, plan, preflight)?;
    let inputs =
        ProtectedInputs::open([executable]).map_err(|_| "无法保护当前参考恢复协调器可执行文件")?;
    let result = execute(command, timeout);
    let executable_check_failed = inputs.assert_unchanged().is_err();
    match result {
        Ok(Some(0)) if !executable_check_failed => Ok(()),
        Ok(status) => Err(ChildFailure {
            status,
            confirmed_closed: true,
            reason: if status == Some(0) {
                "参考数据准备完成未获确认"
            } else {
                "参考数据准备子进程异常退出"
            },
            executable_check_failed,
        }
        .into()),
        Err(mut error) => {
            if executable_check_failed {
                let Some(failure) = error.downcast_mut::<ChildFailure>() else {
                    return Err("参考数据准备子进程未启动，当前协调器可执行文件后验失败".into());
                };
                failure.executable_check_failed = true;
            }
            Err(error)
        }
    }
}

fn command(executable: &Path, backend: &Path, plan: &Path, preflight: &Path) -> Result<Command> {
    let [backend_text, plan, preflight] = [backend, plan, preflight].map(Path::to_str);
    let payload = super::super::protocol::dataset(&json!({
        "backend": backend_text,
        "plan": plan,
        "preflight": preflight,
    }))
    .map_err(|_| "参考数据准备的有界私有请求无效")?;
    let payload = payload
        .as_str()
        .ok_or("参考数据准备的私有请求必须为 UTF-8 文本")?;
    #[cfg(not(windows))]
    {
        let stdin = protocol_stdin(Some(payload.as_bytes()))
            .map_err(|_| "无法准备参考数据子进程的受管标准输入")?;
        let mut command = Command::new(executable);
        restrict_environment(&mut command, false);
        command
            .arg(MARKER)
            .current_dir(backend)
            .stdin(stdin)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Ok(command)
    }
    #[cfg(windows)]
    {
        Ok(Command::new(executable)
            .arg(MARKER)
            .current_dir(backend)
            .env_clear()
            .envs(system_environment())
            .stdin(Stdin::from_bytes(payload.as_bytes()))
            .stdout(StdioMode::Null)
            .stderr(StdioMode::Null)
            .create_no_window())
    }
}

#[cfg(not(windows))]
fn execute(command: Command, timeout: Duration) -> Result<Option<i32>> {
    let output = command_output_result_with_timeout(command, timeout)
        .map_err(|error| ChildFailure::managed(error.as_ref()))?;
    Ok(output.status.code())
}

#[cfg(windows)]
fn execute(command: Command, timeout: Duration) -> Result<Option<i32>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "无法建立参考数据准备子进程运行环境")?;
    let group = ProcessGroup::new().map_err(|_| "无法建立参考数据准备子进程的专属 Job")?;
    let command = command.timeout(timeout);
    let outcome = runtime.block_on(async { group.start(&command).await?.wait().await });
    // 共享 Job 的 wait 只收割直接子进程。成功与失败都要复核独占 Job 的整树退出。
    if close_owned_windows_job(&group).is_err() {
        let status = outcome.as_ref().ok().and_then(Outcome::code);
        return Err(ChildFailure::unconfirmed(status, "参考数据准备子进程整树回收未获确认").into());
    }
    match outcome {
        Ok(Outcome::Exited(status)) => Ok(Some(status)),
        Ok(outcome) => Err(ChildFailure {
            status: outcome.code(),
            confirmed_closed: true,
            reason: if outcome.timed_out() {
                "参考数据准备子进程达到阶段时限"
            } else {
                "参考数据准备子进程异常退出"
            },
            executable_check_failed: false,
        }
        .into()),
        Err(error) => Err(ChildFailure::managed(&error).into()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[cfg(windows)]
    #[test]
    fn dataset_windows_child_confirms_the_original_job_after_real_process_exits() -> Result<()> {
        let executable = std::env::current_exe()?;
        for argument in ["--help", "--ryframe-unknown-libtest-option"] {
            // 两个 libtest 参数都立即退出，不执行其他测试或依赖命令行解释器。
            let direct = std::process::Command::new(&executable)
                .arg(argument)
                .output()?;
            assert_eq!(direct.status.success(), argument == "--help");
            let command = Command::new(&executable)
                .arg(argument)
                .env_clear()
                .envs(system_environment())
                .stdin(Stdin::from_bytes(b""))
                .stdout(StdioMode::Null)
                .stderr(StdioMode::Null)
                .create_no_window();
            assert_eq!(
                execute(command, Duration::from_secs(5))?,
                direct.status.code()
            );
        }
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn dataset_windows_timeout_closes_job_and_preserves_failed_stage() -> Result<()> {
        use super::super::super::tests::Fixture;
        use super::super::{Lock, Stage, native, receipts, require_absent};

        const CHILD_MARKER: &str = "ryframe-reference-dataset-timeout-child";
        if std::env::args_os().any(|arg| arg == CHILD_MARKER) {
            std::thread::sleep(Duration::from_secs(30));
            return Ok(());
        }

        let fixture = Fixture::new();
        let work = Path::new(fixture.plan["work_dir"].as_str().unwrap());
        let lock = Lock::claim(work)?;
        let args = json!({"plan":fixture.plan,"command":"dataset","copy_id":null,
            "root":null,"receipt":null,"bindings":null});
        let started = receipts::start(&args)?;
        let stage = Stage {
            backend: work.parent().unwrap(),
            work,
            lock: &lock,
            started: &started,
            plan: &fixture.plan,
        };
        let name = format!(
            "{}::dataset_windows_timeout_closes_job_and_preserves_failed_stage",
            module_path!().split_once("::").unwrap().1
        );
        let command = Command::new(std::env::current_exe()?)
            .args(["--exact", &name, "--skip", CHILD_MARKER])
            .env_clear()
            .envs(system_environment())
            .stdin(Stdin::from_bytes(b""))
            .stdout(StdioMode::Null)
            .stderr(StdioMode::Null)
            .create_no_window();
        let error = execute(command, Duration::from_secs(2)).unwrap_err();
        let failure = error.downcast_ref::<ChildFailure>().unwrap();
        assert!(failure.confirmed_closed);
        assert!(failure.to_string().contains("达到阶段时限"));

        let saved_error = stage.finish(Err(error)).unwrap_err();
        assert!(
            saved_error
                .downcast_ref::<ChildFailure>()
                .unwrap()
                .confirmed_closed
        );
        let path = work.join("dataset.json");
        let original = std::fs::read(&path)?;
        let receipt = native::json::read(&path)?;
        assert_eq!(receipt["status"], "failed");
        assert!(receipt.get("result").is_none());
        assert!(
            receipt["failure"]["notes"][0]
                .as_str()
                .unwrap()
                .contains("达到阶段时限")
        );
        assert!(require_absent(&path).is_err());
        assert!(receipts::start(&args).is_err());
        assert_eq!(std::fs::read(path)?, original);
        Ok(())
    }

    #[test]
    fn dataset_child_keeps_request_paths_out_of_argv_and_discards_raw_diagnostics() {
        let backend = std::env::current_dir().unwrap();
        let executable = backend.join("bound-xtask");
        let command = command(
            &executable,
            &backend,
            &backend.join("plan.json"),
            &backend.join("preflight.json"),
        )
        .unwrap();
        #[cfg(not(windows))]
        let (program, args, cwd, mut keys) = (
            command.get_program(),
            command.get_args().collect::<Vec<_>>(),
            command.get_current_dir(),
            command.get_envs().map(|(key, _)| key),
        );
        #[cfg(windows)]
        let (program, args, cwd, mut keys) = (
            command.program(),
            command
                .arguments()
                .iter()
                .map(|arg| arg.as_os_str())
                .collect::<Vec<_>>(),
            command.working_dir(),
            command
                .env_overrides()
                .iter()
                .map(|(key, _)| key.as_os_str()),
        );
        assert_eq!(program, executable.as_os_str());
        assert_eq!(args, [OsStr::new(MARKER)]);
        assert_eq!(cwd, Some(backend.as_path()));
        assert!(keys.all(|key| {
            let key = key.to_string_lossy().to_ascii_uppercase();
            !key.starts_with("APP_")
                && !key.starts_with("RYFRAME_")
                && !key.starts_with("PYTHON")
                && !key.ends_with("_PROXY")
        }));

        let raw = crate::process::PreservedFailure::new("private fixture value".into(), Some(9));
        let failure = ChildFailure::managed(&raw);
        assert_eq!(failure.status, Some(9));
        assert!(!failure.confirmed_closed);
        assert!(failure.to_string().contains("退出码 9"));
        assert!(!failure.to_string().contains("private fixture value"));
        assert!(!format!("{failure:?}").contains("private fixture value"));
    }
}

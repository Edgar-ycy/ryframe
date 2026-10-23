use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use crate::{Result, workspace::root_dir};

#[path = "process/child.rs"]
mod child;
#[cfg(windows)]
#[path = "process/windows_members.rs"]
mod windows_members;
pub(crate) use child::{ChildGroup, ManagedChild};
#[path = "process/cancellation.rs"]
mod cancellation;
#[allow(unused_imports)]
pub(crate) use cancellation::{
    ProcessCancellation, is_process_cancellation, with_process_cancellation,
};
use cancellation::{command_output_result, command_output_result_with_timeout, command_status};
#[path = "process/cache.rs"]
mod cache;
use cache::configure_cargo_cache;
#[allow(unused_imports)]
pub(crate) use cache::rustc_cache_label;
#[path = "process/exit_status.rs"]
mod exit_status;
use exit_status::CommandFailure;
#[allow(unused_imports)]
pub(crate) use exit_status::PreservedFailure;
#[allow(unused_imports)]
pub(crate) use exit_status::failure_exit_code;
#[path = "process/logging.rs"]
mod logging;
#[allow(unused_imports)]
pub(crate) use logging::with_process_log;
use logging::{configure_output, process_log_active};
#[path = "process/redirect.rs"]
mod redirect;
#[allow(unused_imports)]
pub(crate) use redirect::run_owned_with_env_redirected;
#[path = "process/python.rs"]
mod python;
use python::configure_environment;
#[allow(unused_imports)]
pub(crate) use python::{python_arguments, python_environment};

type StepObserver = fn(String, f64, bool);
static STEP_OBSERVER: OnceLock<StepObserver> = OnceLock::new();

pub(crate) fn install_step_observer(observer: StepObserver) {
    let _ = STEP_OBSERVER.set(observer);
}

fn record_step(label: String, elapsed: f64, succeeded: bool) {
    if let Some(observer) = STEP_OBSERVER.get() {
        observer(label, elapsed, succeeded);
    }
}

#[cfg(windows)]
const COREPACK_EXECUTABLE: &str = "corepack.cmd";
#[cfg(not(windows))]
const COREPACK_EXECUTABLE: &str = "corepack";

pub(crate) fn run(dir: &Path, executable: &str, args: &[&str]) -> Result<()> {
    run_with_env(dir, executable, args, &[])
}

pub(crate) fn run_owned(dir: &Path, executable: &str, args: &[String]) -> Result<()> {
    run_owned_with_env(dir, executable, args, &[])
}

pub(crate) fn run_owned_with_env(
    dir: &Path,
    executable: &str,
    args: &[String],
    environment: &[(&'static str, String)],
) -> Result<()> {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let environment = environment
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    run_with_env(dir, executable, &args, &environment)
}

pub(crate) fn run_with_env(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<()> {
    run_with_env_removed(dir, executable, args, environment, &[])
}

pub(crate) fn run_with_env_removed(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    removed_environment: &[&str],
) -> Result<()> {
    run_with_stdio(
        dir,
        executable,
        args,
        environment,
        removed_environment,
        Stdio::inherit(),
    )
}

pub(crate) fn run_with_stdin(
    dir: &Path,
    executable: &str,
    args: &[&str],
    input: Stdio,
) -> Result<()> {
    run_with_stdio(dir, executable, args, &[], &[], input)
}

fn run_with_stdio(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    removed_environment: &[&str],
    input: Stdio,
) -> Result<()> {
    let started = Instant::now();
    let logged = process_log_active();
    let command_executable = resolved_executable(&root_dir(), executable)?;
    let command_label = command_executable.to_string_lossy();
    let args = python_arguments(executable, args);
    if !logged {
        println!("→ {command_label} {}", args.join(" "));
    }
    let mut command = child_command(&command_executable);
    configure_cargo_cache(executable, &mut command);
    command.args(&args);
    configure_environment(&mut command, executable, environment, removed_environment);
    command.current_dir(dir).stdin(input);
    configure_output(&mut command)?;
    let status = command_status(command);
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("{command_label} {}", args.join(" "));
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            record_step(label, elapsed, false);
            return Err(error);
        }
    };
    record_step(label, elapsed, status.success());
    if status.success() {
        if !logged {
            println!("✓ {elapsed:.1}s");
        }
        Ok(())
    } else {
        Err(CommandFailure::new(
            format!(
                "命令执行失败（{elapsed:.1}s）：{command_label} {}",
                args.join(" ")
            ),
            status,
        )
        .into())
    }
}

pub(crate) fn run_pnpm(dir: &Path, args: &[&str]) -> Result<()> {
    run_pnpm_with_env(dir, args, &[])
}

pub(crate) fn run_pnpm_with_env(
    dir: &Path,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<()> {
    let started = Instant::now();
    let logged = process_log_active();
    if !logged {
        println!("→ pnpm {}", args.join(" "));
    }
    let mut command = pnpm_command(dir)?;
    configure_pnpm_environment(&mut command, environment);
    command.args(args).stdin(Stdio::inherit());
    configure_output(&mut command)?;
    let status = command_status(command);
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("pnpm {}", args.join(" "));
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            record_step(label, elapsed, false);
            return Err(error);
        }
    };
    record_step(label, elapsed, status.success());
    if status.success() {
        if !logged {
            println!("✓ {elapsed:.1}s");
        }
        Ok(())
    } else {
        Err(CommandFailure::new(
            format!("命令执行失败（{elapsed:.1}s）：pnpm {}", args.join(" ")),
            status,
        )
        .into())
    }
}

pub(crate) fn configure_pnpm_environment(command: &mut Command, environment: &[(&str, &str)]) {
    // verify 会把输出写入独立日志，pnpm 此时没有终端。显式使用 CI 模式可禁止
    // Corepack 在执行脚本前尝试交互式重建 node_modules。
    if !environment.iter().any(|(key, _)| *key == "CI") {
        command.env("CI", "true");
    }
    command.envs(environment.iter().copied());
}

pub(crate) fn command_output(dir: &Path, executable: &str, args: &[&str]) -> Result<String> {
    command_output_with_env(dir, executable, args, &[])
}

pub(crate) fn command_output_quiet(dir: &Path, executable: &str, args: &[&str]) -> Result<String> {
    command_output_impl(dir, executable, args, &[], (false, false), None)
}

pub(crate) fn command_output_with_env(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<String> {
    command_output_impl(dir, executable, args, environment, (true, true), None)
}

fn command_output_impl(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    (visible, configure_cache): (bool, bool),
    input: Option<&[u8]>,
) -> Result<String> {
    let started = Instant::now();
    let logged = process_log_active();
    let command_executable = resolved_executable(&root_dir(), executable)?;
    let command_label = command_executable.to_string_lossy();
    let args = python_arguments(executable, args);
    if visible && !logged {
        println!("→ {command_label} {}", args.join(" "));
    }
    let mut command = child_command(&command_executable);
    if configure_cache {
        configure_cargo_cache(executable, &mut command);
    }
    configure_environment(&mut command, executable, environment, &[]);
    command
        .args(&args)
        .current_dir(dir)
        .stdin(protocol_stdin(input)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command_output_result(command)?;
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("{command_label} {}", args.join(" "));
    record_step(label, elapsed, output.status.success());
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(CommandFailure::new(
            format!(
                "命令执行失败（{elapsed:.1}s）：{command_label} {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" ")
            ),
            output.status,
        )
        .into());
    }
    if visible && !logged {
        println!("✓ {elapsed:.1}s");
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{executable} 输出不是 UTF-8：{error}").into())
}

/// 执行一次性 stdin/output 协议命令，并在超时后回收完整进程树。
pub(crate) fn command_output_with_input_timeout(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output> {
    let command_executable = resolved_executable(&root_dir(), executable)?;
    let args = python_arguments(executable, args);
    let mut command = child_command(&command_executable);
    configure_cargo_cache(executable, &mut command);
    configure_environment(&mut command, executable, environment, &[]);
    command.args(&args).current_dir(dir);
    command.stdin(protocol_stdin(input)?);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command_output_result_with_timeout(command, timeout).map_err(|error| {
        format!(
            "无法执行一次性 Python 输出协议 `{}`：{error}",
            command_executable.to_string_lossy()
        )
        .into()
    })
}

/// 为一次性私有协议准备可关闭的 stdin，不使用环境变量或写入线程。
pub(crate) fn protocol_stdin(input: Option<&[u8]>) -> Result<Stdio> {
    let Some(input) = input else {
        return Ok(Stdio::null());
    };
    let mut file = tempfile::tempfile()?;
    file.write_all(input)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(Stdio::from(file))
}

/// 捕获长时私有阶段的输出，沿用进程树取消规则，不设置总运行期限。
pub(crate) fn command_output_with_input(
    dir: &Path,
    executable: &str,
    args: &[&str],
    input: &[u8],
) -> Result<String> {
    command_output_impl(dir, executable, args, &[], (true, true), Some(input))
}

pub(crate) fn resolved_executable(root: &Path, executable: &str) -> Result<OsString> {
    let field = match executable {
        "python" => "python_program",
        "node" => "node_program",
        _ => return Ok(OsString::from(executable)),
    };
    let repair = format!(
        "verification.{field} 无效；请检查 config/tools.toml 与 .local-tests/tools.toml，显式入口须非空、无控制字符和父目录跳转，相对路径以后端根目录解析"
    );
    let settings =
        ryframe_config::ToolSettings::load(root).map_err(|error| format!("{error}；{repair}"))?;
    let configured = if executable == "python" {
        settings.verification.python_program
    } else {
        settings.verification.node_program
    };
    let Some(path) = configured else {
        return Ok(OsString::from(executable));
    };
    if !path.to_str().is_some_and(|text| {
        !text.is_empty()
            && !text.chars().any(char::is_control)
            && !text.split(['/', '\\']).any(|part| part == "..")
    }) || path
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(repair.into());
    }
    if path.is_absolute() {
        return Ok(path.into_os_string());
    }
    std::path::absolute(root.join(path))
        .map(PathBuf::into_os_string)
        .map_err(|_| repair.into())
}

/// 仓库任务不能把 xtask 自身的 Cargo 包上下文泄漏给嵌套命令。
/// 部分依赖的构建脚本会跟踪这些变量，泄漏后会让完全相同的构建缓存反复失效。
pub(crate) fn child_command(executable: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(executable);
    command
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_MANIFEST_PATH");
    command
}

pub(crate) fn command_version_output(dir: &Path, executable: &str) -> Result<String> {
    command_output(dir, executable, &["--version"])
}

pub(crate) fn pnpm_version_output(dir: &Path) -> Result<String> {
    let output = pnpm_command(dir)?.arg("--version").output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("无法通过项目级 Corepack shim 获取 pnpm 版本：{stderr}").into());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("pnpm --version 输出不是 UTF-8：{error}").into())
}

fn pnpm_command(dir: &Path) -> Result<Command> {
    let executable = pnpm_executable(dir)?;
    let shim_dir = executable
        .parent()
        .expect("pnpm Corepack shim 必须具有父目录");
    let inherited_path = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![shim_dir.to_path_buf()];
    paths.extend(env::split_paths(&inherited_path));
    let path = env::join_paths(paths)
        .map_err(|error| format!("无法构造 pnpm 的 PATH 环境变量：{error}"))?;

    let mut command = Command::new(executable);
    command.current_dir(dir).env("PATH", path);
    Ok(command)
}

fn pnpm_executable(dir: &Path) -> Result<PathBuf> {
    let shim_dir = root_dir().join("target").join("corepack-bin");
    fs::create_dir_all(&shim_dir).map_err(|error| {
        format!(
            "无法创建 Corepack shim 目录 {}：{error}",
            shim_dir.display()
        )
    })?;

    #[cfg(windows)]
    let executable = shim_dir.join("pnpm.cmd");
    #[cfg(not(windows))]
    let executable = shim_dir.join("pnpm");

    if !executable.is_file() {
        let mut command = Command::new(COREPACK_EXECUTABLE);
        command
            .args(["enable", "--install-directory"])
            .arg(&shim_dir)
            .current_dir(dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = command_status(command)?;
        if !status.success() {
            return Err("无法创建项目级 Corepack pnpm shim".into());
        }
    }

    if !executable.is_file() {
        return Err(format!("Corepack 未创建 pnpm shim：{}", executable.display()).into());
    }
    Ok(executable)
}

#[cfg(unix)]
fn prepare_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // 为每个开发服务建立独立进程组，停止时能同时回收其派生子进程。
    command.process_group(0);
}

pub(crate) fn stop_child(child: &mut ManagedChild) -> Result<()> {
    #[cfg(windows)]
    {
        let members = child.member_handles()?;
        child.terminate_tree()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let direct_exited = child.try_wait()?.is_some();
            let tree_exited = child.active_process_count()? == Some(0);
            if direct_exited && tree_exited && members.all_exited()? {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Windows 子进程树终止后 1 秒内仍有存活进程".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::Signal;

        const TERMINATE_GRACE: Duration = Duration::from_millis(400);
        const FORCE_KILL_GRACE: Duration = Duration::from_millis(400);

        let process_group_id = child.process_group_id();
        signal_process_group(process_group_id, Signal::SIGTERM)?;
        if !wait_for_process_group_exit(child, process_group_id, TERMINATE_GRACE)? {
            signal_process_group(process_group_id, Signal::SIGKILL)?;
            if !wait_for_process_group_exit(child, process_group_id, FORCE_KILL_GRACE)? {
                let _ = child.kill();
                return Err("Unix 子进程组强制终止后仍未确认退出".into());
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    if child.try_wait()?.is_none() {
        child.kill()?;
    }

    let _ = child.wait();
    Ok(())
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: nix::sys::signal::Signal) -> Result<()> {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

    let pid = i32::try_from(pid).map_err(|_| "子进程 ID 超出 POSIX 范围")?;
    // POSIX 规定负 PID 表示整个进程组；子进程在 spawn 时成为自己的进程组组长。
    match kill(Pid::from_raw(-pid), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => {
            let error = std::io::Error::from(error);
            Err(format!("无法向子进程组 {pid} 发送信号：{error}").into())
        }
    }
}

#[cfg(unix)]
fn wait_for_process_group_exit(
    child: &mut ManagedChild,
    process_group_id: u32,
    timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let _ = child.try_wait()?;
        if !process_group_exists(process_group_id)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn process_group_exists(process_group_id: u32) -> Result<bool> {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

    let process_group_id =
        i32::try_from(process_group_id).map_err(|_| "子进程组 ID 超出 POSIX 范围")?;
    match kill(Pid::from_raw(-process_group_id), None) {
        Ok(()) | Err(Errno::EPERM) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => {
            let error = std::io::Error::from(error);
            Err(format!("无法查询子进程组 {process_group_id}：{error}").into())
        }
    }
}

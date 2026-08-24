use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Once, OnceLock},
    thread,
    time::{Duration, Instant},
};

use crate::{Result, workspace::root_dir};

#[path = "process/child.rs"]
mod child;
pub(crate) use child::ChildGroup;
#[path = "process/logging.rs"]
mod logging;
#[allow(unused_imports)]
pub(crate) use logging::with_process_log;
use logging::{configure_output, process_log_active};

enum RustcCache {
    ExistingWrapper(OsString),
    Sccache,
    Cargo,
}

pub(crate) fn rustc_cache_label() -> &'static str {
    match RUSTC_CACHE.get_or_init(resolve_rustc_cache) {
        RustcCache::ExistingWrapper(_) => "existing-wrapper",
        RustcCache::Sccache => "sccache",
        RustcCache::Cargo => "cargo",
    }
}

static RUSTC_CACHE: OnceLock<RustcCache> = OnceLock::new();
static RUSTC_CACHE_NOTICE: Once = Once::new();
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

pub(crate) fn run_with_env(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<()> {
    let started = Instant::now();
    let logged = process_log_active();
    if !logged {
        println!("→ {executable} {}", args.join(" "));
    }
    let mut command = child_command(executable);
    configure_cargo_cache(executable, &mut command);
    command
        .args(args)
        .envs(environment.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::inherit());
    configure_output(&mut command)?;
    let status = command.status();
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("{executable} {}", args.join(" "));
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            record_step(label, elapsed, false);
            return Err(error.into());
        }
    };
    record_step(label, elapsed, status.success());
    if status.success() {
        if !logged {
            println!("✓ {elapsed:.1}s");
        }
        Ok(())
    } else {
        Err(format!(
            "命令执行失败（{elapsed:.1}s）：{executable} {}",
            args.join(" ")
        )
        .into())
    }
}

fn configure_cargo_cache(executable: &str, command: &mut Command) {
    if executable != "cargo" {
        return;
    }
    match RUSTC_CACHE.get_or_init(resolve_rustc_cache) {
        RustcCache::ExistingWrapper(wrapper) => {
            RUSTC_CACHE_NOTICE.call_once(|| {
                println!(
                    "检测到 RUSTC_WRAPPER={}，保留现有编译缓存配置。",
                    wrapper.to_string_lossy()
                );
            });
        }
        RustcCache::Sccache => {
            command.env("RUSTC_WRAPPER", "sccache");
            RUSTC_CACHE_NOTICE.call_once(|| println!("使用 sccache 复用 Rust 编译缓存。"));
        }
        RustcCache::Cargo => {
            RUSTC_CACHE_NOTICE.call_once(|| {
                println!("未检测到可正常运行的 sccache，使用 Cargo 本地缓存继续执行。");
            });
        }
    }
}

fn resolve_rustc_cache() -> RustcCache {
    if let Some(wrapper) = env::var_os("RUSTC_WRAPPER").filter(|value| !value.is_empty()) {
        return RustcCache::ExistingWrapper(wrapper);
    }
    let available = sccache_can_wrap_rustc();
    if available {
        RustcCache::Sccache
    } else {
        RustcCache::Cargo
    }
}

/// sccache 能被找到不代表其服务可以启动。用一次短暂的真实编译器调用探测，
/// 避免远端缓存或后台服务故障让开发命令整体失败。
fn sccache_can_wrap_rustc() -> bool {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let mut child = match Command::new("sccache")
        .arg(rustc)
        .arg("-vV")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
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
    command
        .args(args)
        .envs(environment.iter().copied())
        .stdin(Stdio::inherit());
    configure_output(&mut command)?;
    let status = command.status();
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("pnpm {}", args.join(" "));
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            record_step(label, elapsed, false);
            return Err(error.into());
        }
    };
    record_step(label, elapsed, status.success());
    if status.success() {
        if !logged {
            println!("✓ {elapsed:.1}s");
        }
        Ok(())
    } else {
        Err(format!("命令执行失败（{elapsed:.1}s）：pnpm {}", args.join(" ")).into())
    }
}

pub(crate) fn command_output(dir: &Path, executable: &str, args: &[&str]) -> Result<String> {
    command_output_with_env(dir, executable, args, &[])
}

pub(crate) fn command_output_with_env(
    dir: &Path,
    executable: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<String> {
    let started = Instant::now();
    let logged = process_log_active();
    if !logged {
        println!("→ {executable} {}", args.join(" "));
    }
    let mut command = child_command(executable);
    configure_cargo_cache(executable, &mut command);
    let output = command
        .args(args)
        .envs(environment.iter().copied())
        .current_dir(dir)
        .output()?;
    let elapsed = started.elapsed().as_secs_f64();
    let label = format!("{executable} {}", args.join(" "));
    record_step(label, elapsed, output.status.success());
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!(
            "命令执行失败（{elapsed:.1}s）：{executable} {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            args.join(" ")
        )
        .into());
    }
    if !logged {
        println!("✓ {elapsed:.1}s");
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{executable} 输出不是 UTF-8：{error}").into())
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
        let status = Command::new(COREPACK_EXECUTABLE)
            .args(["enable", "--install-directory"])
            .arg(&shim_dir)
            .current_dir(dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err("无法创建项目级 Corepack pnpm shim".into());
        }
    }

    if !executable.is_file() {
        return Err(format!("Corepack 未创建 pnpm shim：{}", executable.display()).into());
    }
    Ok(executable)
}

pub(crate) fn spawn_pnpm_with_env(
    group: &ChildGroup,
    dir: &Path,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<Child> {
    let mut command = pnpm_command(dir)?;
    command
        .args(args)
        .envs(environment.iter().copied())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    group.spawn(&mut command)
}

#[cfg(unix)]
fn prepare_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // 为每个开发服务建立独立进程组，停止时能同时回收其派生子进程。
    command.process_group(0);
}

#[cfg(windows)]
fn prepare_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    // 先挂起创建，再加入 Job Object，确保任何业务代码和后代进程都不能抢先运行。
    command.creation_flags(CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn prepare_process_group(_command: &mut Command) {}

pub(crate) fn stop_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let status = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() && child.try_wait()?.is_none() {
            child.kill()?;
        }
    }
    #[cfg(unix)]
    {
        signal_process_group(child.id(), libc::SIGTERM)?;
        if wait_for_child_exit(child, Duration::from_secs(5))? {
            return Ok(());
        }

        signal_process_group(child.id(), libc::SIGKILL)?;
        if !wait_for_child_exit(child, Duration::from_secs(5))? {
            child.kill()?;
        }
    }

    let _ = child.wait();
    Ok(())
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int) -> Result<()> {
    let pid = i32::try_from(pid).map_err(|_| "子进程 ID 超出 POSIX 范围")?;
    // POSIX 规定负 PID 表示整个进程组；子进程在 spawn 时成为自己的进程组组长。
    let result = unsafe { libc::kill(-pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(format!("无法向子进程组 {pid} 发送信号：{error}").into())
}

#[cfg(unix)]
fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

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

enum RustcCache {
    ExistingWrapper(OsString),
    Sccache,
    Cargo,
}

static RUSTC_CACHE: OnceLock<RustcCache> = OnceLock::new();
static RUSTC_CACHE_NOTICE: Once = Once::new();

/// 开发任务的子进程容器。Windows 使用带 `KILL_ON_JOB_CLOSE` 的 Job Object，
/// 即使 xtask 异常退出也会回收 API、Worker、Vite 及其后代进程。
pub(crate) struct ChildGroup {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

impl ChildGroup {
    pub(crate) fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            use std::{ffi::c_void, mem};
            use windows_sys::Win32::{
                Foundation::{CloseHandle, HANDLE},
                System::JobObjects::{
                    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject,
                },
            };

            let job: HANDLE = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(format!(
                    "无法创建 Windows Job Object：{}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const information).cast::<c_void>(),
                    u32::try_from(mem::size_of_val(&information))
                        .expect("Job Object 配置大小必须可由 u32 表示"),
                )
            };
            if configured == 0 {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(format!("无法配置 Windows Job Object：{error}").into());
            }
            Ok(Self { job })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    pub(crate) fn spawn(&self, command: &mut Command) -> Result<Child> {
        prepare_process_group(command);
        let mut child = command.spawn()?;
        #[cfg(windows)]
        {
            if let Err(error) = assign_and_resume_suspended_child(self.job, &child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        Ok(child)
    }
}

#[cfg(windows)]
fn assign_and_resume_suspended_child(
    job: windows_sys::Win32::Foundation::HANDLE,
    child: &Child,
) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    // Child 保有 CreateProcess 返回的真实进程句柄，可避免按 PID 再次打开时的竞态。
    let process = child.as_raw_handle().cast();
    if unsafe { AssignProcessToJobObject(job, process) } == 0 {
        return Err(format!(
            "无法把挂起的子进程加入 Windows Job Object：{}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    resume_initial_thread(child.id())
}

#[cfg(windows)]
fn resume_initial_thread(process_id: u32) -> Result<()> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "无法枚举挂起子进程的初始线程：{}",
            std::io::Error::last_os_error()
        )
        .into());
    }

    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(mem::size_of::<THREADENTRY32>())
                .expect("Windows 线程条目大小必须可由 u32 表示"),
            ..Default::default()
        };
        let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
        let mut last_open_error = None;
        while has_entry {
            if entry.th32OwnerProcessID == process_id {
                let thread: HANDLE =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    last_open_error = Some(std::io::Error::last_os_error());
                } else {
                    let previous_suspend_count = unsafe { ResumeThread(thread) };
                    unsafe { CloseHandle(thread) };
                    if previous_suspend_count == u32::MAX {
                        return Err(format!(
                            "无法恢复挂起子进程 {process_id} 的初始线程：{}",
                            std::io::Error::last_os_error()
                        )
                        .into());
                    }
                    if previous_suspend_count == 1 {
                        return Ok(());
                    }
                    return Err(format!(
                        "挂起子进程 {process_id} 的初始线程挂起计数异常：{previous_suspend_count}"
                    )
                    .into());
                }
            }
            has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
        }
        if let Some(error) = last_open_error {
            Err(format!("无法打开挂起子进程 {process_id} 的初始线程：{error}").into())
        } else {
            Err(format!("未找到挂起子进程 {process_id} 的初始线程").into())
        }
    })();
    unsafe { CloseHandle(snapshot) };
    result
}

#[cfg(windows)]
impl Drop for ChildGroup {
    fn drop(&mut self) {
        if !self.job.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
            self.job = std::ptr::null_mut();
        }
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
    println!("→ {executable} {}", args.join(" "));
    let mut command = child_command(executable);
    configure_cargo_cache(executable, &mut command);
    let status = command
        .args(args)
        .envs(environment.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        println!("✓ {:.1}s", started.elapsed().as_secs_f64());
        Ok(())
    } else {
        Err(format!(
            "命令执行失败（{:.1}s）：{executable} {}",
            started.elapsed().as_secs_f64(),
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
    println!("→ pnpm {}", args.join(" "));
    let status = pnpm_command(dir)?
        .args(args)
        .envs(environment.iter().copied())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        println!("✓ {:.1}s", started.elapsed().as_secs_f64());
        Ok(())
    } else {
        Err(format!(
            "命令执行失败（{:.1}s）：pnpm {}",
            started.elapsed().as_secs_f64(),
            args.join(" ")
        )
        .into())
    }
}

pub(crate) fn command_output(dir: &Path, executable: &str, args: &[&str]) -> Result<String> {
    let output = child_command(executable)
        .args(args)
        .current_dir(dir)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("命令执行失败：{executable} {}：{stderr}", args.join(" ")).into());
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

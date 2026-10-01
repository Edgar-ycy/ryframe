use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nix::sys::statfs::{CGROUP2_SUPER_MAGIC, fstatfs};

use crate::{Result, process::ChildGroup};

use super::MemoryReading;

const CGROUP_ROOT_ENV: &str = "RYFRAME_DEVEX_CGROUP_ROOT";
const TRAMPOLINE_GROUP_ENV: &str = "RYFRAME_DEVEX_CGROUP_TRAMPOLINE";
const GROUP_PREFIX: &str = "ryframe-devex-";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn execute(command: Command) -> Result<(ExitStatus, MemoryReading)> {
    execute_with_trampoline(command, env::current_exe()?)
}

pub(crate) fn execute_with_trampoline(
    command: Command,
    trampoline_executable: PathBuf,
) -> Result<(ExitStatus, MemoryReading)> {
    let cgroup = match Cgroup::prepare() {
        Ok(cgroup) => cgroup,
        Err(error) => {
            let mut child = ChildGroup::new()?.spawn(command)?;
            return Ok((child.wait()?, MemoryReading::unavailable(error.to_string())));
        }
    };
    let trampoline = trampoline_command(command, &cgroup.path, &trampoline_executable)?;
    let mut child = ChildGroup::new()?.spawn(trampoline)?;
    let status = child.wait()?;
    let reading = MemoryReading::from_result(cgroup.peak());
    drop(child);
    cgroup.cleanup()?;
    Ok((status, reading))
}

/// 返回 `Some` 表示当前进程是内部跳板。成功路径用 `exec` 替换当前进程，
/// 因而只有登记或执行失败才会返回给调用方。
pub(crate) fn run_trampoline_if_requested() -> Option<Result<()>> {
    env::var_os(TRAMPOLINE_GROUP_ENV).map(run_trampoline)
}

fn run_trampoline(group: OsString) -> Result<()> {
    let group = validate_owned_group(Path::new(&group))?;
    join_and_verify(&group)?;

    let mut arguments = env::args_os().skip(1);
    let program = arguments.next().ok_or("cgroup 执行跳板缺少目标程序")?;
    let mut command = Command::new(program);
    command.args(arguments).env_remove(TRAMPOLINE_GROUP_ENV);
    let error = command.exec();
    Err(format!("无法 exec 目标程序：{error}").into())
}

fn trampoline_command(command: Command, group: &Path, executable: &Path) -> Result<Command> {
    let program = command.get_program().to_owned();
    let arguments = command.get_args().map(OsStr::to_owned).collect::<Vec<_>>();
    let directory = command.get_current_dir().map(Path::to_owned);
    let environment = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
        .collect::<Vec<_>>();

    let mut trampoline = Command::new(executable);
    trampoline
        .arg(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(directory) = directory {
        trampoline.current_dir(directory);
    }
    for (key, value) in environment {
        if let Some(value) = value {
            trampoline.env(key, value);
        } else {
            trampoline.env_remove(key);
        }
    }
    trampoline.env(TRAMPOLINE_GROUP_ENV, group);
    Ok(trampoline)
}

fn join_and_verify(group: &Path) -> Result<()> {
    let pid = std::process::id().to_string();
    let membership_path = group.join("cgroup.procs");
    let mut membership = OpenOptions::new().write(true).open(&membership_path)?;
    membership.write_all(pid.as_bytes())?;
    membership.flush()?;
    let registered = fs::read_to_string(&membership_path)?
        .lines()
        .any(|entry| entry.trim() == pid);
    if !registered {
        return Err("cgroup.procs 回读未找到执行跳板 PID，拒绝启动目标程序".into());
    }
    Ok(())
}

fn validate_owned_group(group: &Path) -> Result<PathBuf> {
    let root = authorized_root()?;
    let group = fs::canonicalize(group)?;
    if group.parent() != Some(root.as_path()) || !valid_group_name(group.file_name()) {
        return Err("cgroup 执行跳板目标不是本次工具创建的直属隔离组".into());
    }
    Ok(group)
}

pub(crate) fn valid_group_name(name: Option<&OsStr>) -> bool {
    let Some(name) = name.and_then(OsStr::to_str) else {
        return false;
    };
    let Some(identity) = name.strip_prefix(GROUP_PREFIX) else {
        return false;
    };
    let components = identity.split('-').collect::<Vec<_>>();
    components.len() == 3
        && components
            .iter()
            .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
}

fn authorized_root() -> Result<PathBuf> {
    let root = env::var_os(CGROUP_ROOT_ENV).ok_or(
        "未设置 RYFRAME_DEVEX_CGROUP_ROOT；需要显式授权且启用 memory controller 的 cgroup v2 委托目录",
    )?;
    let root = fs::canonicalize(root)?;
    let directory = File::open(&root)?;
    if fstatfs(&directory)?.filesystem_type() != CGROUP2_SUPER_MAGIC {
        return Err("RYFRAME_DEVEX_CGROUP_ROOT 不是 cgroup v2 文件系统".into());
    }
    Ok(root)
}

struct Cgroup {
    path: PathBuf,
}

impl Cgroup {
    fn prepare() -> Result<Self> {
        let root = authorized_root()?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            "{GROUP_PREFIX}{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        let cgroup = Self { path };
        // 不修改父目录的 controller、限额或已有任务；权限不足时明确报告不可用。
        File::open(cgroup.path.join("memory.peak"))?;
        OpenOptions::new()
            .write(true)
            .open(cgroup.path.join("cgroup.kill"))?;
        OpenOptions::new()
            .write(true)
            .open(cgroup.path.join("cgroup.procs"))?;
        Ok(cgroup)
    }

    fn populated(&self) -> Result<bool> {
        let events = fs::read_to_string(self.path.join("cgroup.events"))?;
        match events
            .lines()
            .find_map(|line| line.strip_prefix("populated "))
        {
            Some("0") => Ok(false),
            Some("1") => Ok(true),
            _ => Err("cgroup.events 缺少合法 populated 状态".into()),
        }
    }

    fn peak(&self) -> Result<u64> {
        if self.populated()? {
            return Err("主命令退出后仍有后代运行，内存测量窗口不完整".into());
        }
        Ok(fs::read_to_string(self.path.join("memory.peak"))?
            .trim()
            .parse()?)
    }

    fn cleanup(&self) -> Result<()> {
        if self.populated()? {
            OpenOptions::new()
                .write(true)
                .open(self.path.join("cgroup.kill"))?
                .write_all(b"1")?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.populated()? {
                if Instant::now() >= deadline {
                    return Err("本次隔离 cgroup 的后代未按期退出，保留目录供排查".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        fs::remove_dir(&self.path)?;
        Ok(())
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        if self.path.exists()
            && let Err(error) = self.cleanup()
        {
            eprintln!("DevEx 隔离 cgroup 清理失败：{error}");
        }
    }
}

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::ffi::{OsStrExt, OsStringExt},
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

const TRAMPOLINE_PREFIX: &[u8] = b"__ryframe_devex_cgroup:";
const IN_PLACE_MODE: u8 = b'i';
const EXTERNAL_MODE: u8 = b'e';
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";
const DRIVER_GROUP: &str = "driver";
const MEASURE_GROUP: &str = "measure";
const GATE_PREFIX: &str = "ryframe-devex-ci-";
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
    let mut arguments = env::args_os();
    let marker = arguments.next()?;
    let (in_place, group) = match parse_trampoline_marker(&marker) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(error) => return Some(Err(error)),
    };
    Some(run_trampoline(in_place, group, arguments))
}

fn run_trampoline(
    in_place: bool,
    group: PathBuf,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<()> {
    let group = validate_owned_group(&group)?;
    join_and_verify(&group)?;

    let error = target_command(in_place, arguments)?.exec();
    Err(format!("无法 exec 目标程序：{error}").into())
}

fn target_command(
    in_place: bool,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Command> {
    let mut arguments = arguments.into_iter();
    let program = if in_place {
        env::current_exe()?.into_os_string()
    } else {
        arguments.next().ok_or("cgroup 执行跳板缺少目标程序")?
    };
    let mut command = Command::new(program);
    command.args(arguments);
    for (key, _) in env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.starts_with("RYFRAME_")
            || name.starts_with("VITE_")
            || name.starts_with("SNOWFLAKE_")
            || (name.starts_with("APP_") && name != "APP_ENV")
        {
            command.env_remove(key);
        }
    }
    Ok(command)
}

fn trampoline_command(mut command: Command, group: &Path, executable: &Path) -> Result<Command> {
    if command.get_program() == executable.as_os_str() {
        // 原生入口已绑定 stdin 文件；保留整个 Command，避免重建时丢失句柄或偏移量。
        // argv[0] 只区分跳板模式，结构化输入仍由原 stdin 传递。
        command.arg0(trampoline_marker(IN_PLACE_MODE, group)?);
        return Ok(command);
    }
    let program = command.get_program().to_owned();
    let arguments = command.get_args().map(OsStr::to_owned).collect::<Vec<_>>();
    let directory = command.get_current_dir().map(Path::to_owned);
    let environment = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
        .collect::<Vec<_>>();

    let mut trampoline = Command::new(executable);
    trampoline
        .arg0(trampoline_marker(EXTERNAL_MODE, group)?)
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
    Ok(trampoline)
}

fn trampoline_marker(mode: u8, group: &Path) -> Result<OsString> {
    if !matches!(mode, IN_PLACE_MODE | EXTERNAL_MODE) || !group.is_absolute() {
        return Err("cgroup 执行跳板标记缺少合法模式或绝对隔离组路径".into());
    }
    let mut bytes = Vec::with_capacity(TRAMPOLINE_PREFIX.len() + 2 + group.as_os_str().len());
    bytes.extend_from_slice(TRAMPOLINE_PREFIX);
    bytes.push(mode);
    bytes.push(b':');
    bytes.extend_from_slice(group.as_os_str().as_bytes());
    Ok(OsString::from_vec(bytes))
}

fn parse_trampoline_marker(marker: &OsStr) -> Result<Option<(bool, PathBuf)>> {
    let bytes = marker.as_bytes();
    if !bytes.starts_with(TRAMPOLINE_PREFIX) {
        return Ok(None);
    }
    let remainder = &bytes[TRAMPOLINE_PREFIX.len()..];
    if remainder.len() < 3 || remainder[1] != b':' {
        return Err("cgroup 执行跳板参数格式非法".into());
    }
    let in_place = match remainder[0] {
        IN_PLACE_MODE => true,
        EXTERNAL_MODE => false,
        _ => return Err("cgroup 执行跳板参数包含未知模式".into()),
    };
    let group = PathBuf::from(OsString::from_vec(remainder[2..].to_vec()));
    if !group.is_absolute() {
        return Err("cgroup 执行跳板参数必须包含绝对隔离组路径".into());
    }
    Ok(Some((in_place, group)))
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

pub(crate) fn authorized_root() -> Result<PathBuf> {
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("当前进程没有可核验的 cgroup v2 统一层级归属")?;
    let mount = fs::canonicalize(CGROUP_MOUNT)?;
    let driver = fs::canonicalize(mount.join(relative.trim_start_matches('/')))?;
    if !driver.starts_with(&mount) {
        return Err("当前进程的 cgroup v2 归属越过统一层级挂载点".into());
    }
    if driver.file_name() != Some(OsStr::new(DRIVER_GROUP)) {
        return Err(
            "当前进程不在 RyFrame cgroup 门禁的 driver 隔离组中，无法提供完整命令树内存证据".into(),
        );
    }
    let gate = driver
        .parent()
        .ok_or("RyFrame cgroup driver 缺少门禁父目录")?;
    let gate_name = gate
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("RyFrame cgroup 门禁目录名不是 UTF-8")?;
    if !valid_gate_name(gate_name) {
        return Err("当前 cgroup driver 不属于 RyFrame 创建的门禁目录".into());
    }
    let root = fs::canonicalize(gate.join(MEASURE_GROUP))?;
    if root.parent() != Some(gate) {
        return Err("RyFrame cgroup measure 目录越过本次门禁边界".into());
    }
    let directory = File::open(&root)?;
    if fstatfs(&directory)?.filesystem_type() != CGROUP2_SUPER_MAGIC {
        return Err("RyFrame cgroup measure 目录不是 cgroup v2 文件系统".into());
    }
    Ok(root)
}

fn valid_gate_name(name: &str) -> bool {
    let Some(identity) = name.strip_prefix(GATE_PREFIX) else {
        return false;
    };
    let mut components = identity.split('-');
    let run_id = components.next().unwrap_or_default();
    let attempt = components.next().unwrap_or_default();
    let nonce = components.next().unwrap_or_default();
    components.next().is_none()
        && decimal_identity(run_id)
        && decimal_identity(attempt)
        && nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn decimal_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() && (index != 0 || byte != b'0'))
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

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use super::super::{
    error::{GateError, GateResult},
    evidence::{KernelIdentity, ParentState, parent_state_from},
    processes::{ProcessFact, ProcessSnapshot},
};

const FREEZE_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn kernel_identity(path: &Path) -> GateResult<KernelIdentity> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| GateError::io("cgroup", "读取 cgroup 内核身份", path, &error))?;
    if metadata.file_type().is_symlink() {
        return Err(GateError::new(
            "cgroup",
            format!("拒绝把符号链接当作本次 cgroup：{}", path.display()),
        ));
    }
    Ok(KernelIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub(super) fn parent_state(cgroup_root: &Path) -> GateResult<ParentState> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").map_err(|error| {
        GateError::new(
            "cgroup",
            format!("读取 /proc/self/mountinfo 以核验 cgroup v2 挂载失败：{error}"),
        )
    })?;
    let subtree =
        fs::read_to_string(cgroup_root.join("cgroup.subtree_control")).map_err(|error| {
            GateError::io(
                "cgroup",
                "读取父 cgroup 的 controller 状态",
                &cgroup_root.join("cgroup.subtree_control"),
                &error,
            )
        })?;
    let identity = kernel_identity(cgroup_root)?;
    parent_state_from(&mountinfo, &subtree, cgroup_root, identity)
}

pub(super) fn write_control(path: &Path, value: &str) -> GateResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| GateError::io("cgroup", "打开 cgroup 控制文件", path, &error))?;
    file.write_all(value.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| GateError::io("cgroup", "写入 cgroup 控制文件", path, &error))
}

pub(super) fn directories(root: &Path) -> GateResult<Vec<PathBuf>> {
    let mut directories = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && directory != root => {
                continue;
            }
            Err(error) => {
                return Err(GateError::io(
                    "cgroup",
                    "扫描 cgroup 子树",
                    &directory,
                    &error,
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                GateError::new(
                    "cgroup",
                    format!(
                        "读取 cgroup 子树 {} 的目录项失败：{error}",
                        directory.display()
                    ),
                )
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                GateError::io("cgroup", "读取 cgroup 子路径类型", &path, &error)
            })?;
            if file_type.is_symlink() {
                return Err(GateError::new(
                    "cgroup",
                    format!("cgroup 子路径中出现符号链接，拒绝越界：{}", path.display()),
                ));
            }
            if file_type.is_dir() {
                directories.push(path.clone());
                stack.push(path);
            }
        }
    }
    Ok(directories)
}

pub(super) fn processes(root: &Path) -> GateResult<ProcessSnapshot> {
    let mut result = BTreeMap::new();
    for directory in directories(root)? {
        let path = directory.join("cgroup.procs");
        let members = match fs::read_to_string(&path) {
            Ok(members) => members,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && directory != root => {
                continue;
            }
            Err(error) => {
                return Err(GateError::io(
                    "cgroup",
                    "读取 cgroup 进程列表",
                    &path,
                    &error,
                ));
            }
        };
        for member in members.split_whitespace() {
            let pid = member.parse::<u32>().map_err(|error| {
                GateError::new(
                    "process",
                    format!(
                        "cgroup 进程列表 {} 包含非法 PID {member:?}：{error}",
                        path.display()
                    ),
                )
            })?;
            if let Some(fact) = process_fact(pid)? {
                result.insert(pid, fact);
            }
        }
    }
    Ok(result)
}

pub(super) fn process_fact(pid: u32) -> GateResult<Option<ProcessFact>> {
    let base = Path::new("/proc").join(pid.to_string());
    let stat = match fs::read_to_string(base.join("stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(GateError::io(
                "process",
                "读取进程 stat",
                &base.join("stat"),
                &error,
            ));
        }
    };
    let end = stat
        .rfind(')')
        .ok_or_else(|| GateError::new("process", format!("进程 {pid} 的 stat 缺少右括号")))?;
    let fields = stat[end + 1..].split_whitespace().collect::<Vec<_>>();
    let parent = fields
        .get(1)
        .ok_or_else(|| GateError::new("process", format!("进程 {pid} 的 stat 缺少父进程")))?
        .parse::<u32>()
        .map_err(|error| {
            GateError::new(
                "process",
                format!("进程 {pid} 的父进程不是合法 PID：{error}"),
            )
        })?;
    let started = fields
        .get(19)
        .ok_or_else(|| GateError::new("process", format!("进程 {pid} 的 stat 缺少启动时刻")))?
        .to_string();
    let status = match fs::read_to_string(base.join("status")) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(GateError::io(
                "process",
                "读取进程 status",
                &base.join("status"),
                &error,
            ));
        }
    };
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|value| value.split_whitespace().next())
        .ok_or_else(|| GateError::new("process", format!("进程 {pid} 的 status 缺少 Uid")))?
        .parse::<u32>()
        .map_err(|error| GateError::new("process", format!("进程 {pid} 的 uid 非法：{error}")))?;
    Ok(Some(ProcessFact {
        pid,
        started,
        parent,
        uid,
    }))
}

pub(super) fn freeze(root: &Path) -> GateResult<BTreeMap<String, String>> {
    write_control(&root.join("cgroup.freeze"), "1")?;
    let deadline = Instant::now() + FREEZE_TIMEOUT;
    loop {
        let events = fs::read_to_string(root.join("cgroup.events")).map_err(|error| {
            GateError::io(
                "cgroup",
                "读取冻结状态",
                &root.join("cgroup.events"),
                &error,
            )
        })?;
        let events = super::super::evidence::parse_events(&events)?;
        match events.get("frozen").map(String::as_str) {
            Some("1") => return Ok(events),
            Some("0") if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Some("0") => {
                return Err(GateError::new(
                    "timeout",
                    "本 job 的 cgroup 在 5 秒内未确认冻结；拒绝在进程仍可能 fork 时清理",
                ));
            }
            Some(value) => {
                return Err(GateError::new(
                    "cgroup",
                    format!("cgroup.events 的 frozen 状态非法：{value:?}"),
                ));
            }
            None => {
                return Err(GateError::new(
                    "cgroup",
                    "cgroup.events 缺少 frozen 字段，拒绝继续清理",
                ));
            }
        }
    }
}

pub(super) fn wait_until_empty(root: &Path, timeout: Duration) -> GateResult<()> {
    let deadline = Instant::now() + timeout;
    while !processes(root)?.is_empty() {
        if Instant::now() >= deadline {
            return Err(GateError::new(
                "timeout",
                format!(
                    "本 job 已登记后代未在 {} 秒内退出；保留精确 cgroup 供排查",
                    timeout.as_secs()
                ),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

pub(super) fn remove_directories(root: &Path) -> GateResult<()> {
    let mut directories = directories(root)?;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GateError::io(
                    "cgroup",
                    "删除精确 cgroup 目录",
                    &directory,
                    &error,
                ));
            }
        }
    }
    Ok(())
}

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    error::{GateError, GateResult},
    evidence,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessFact {
    pub(crate) pid: u32,
    pub(crate) started: String,
    pub(crate) parent: u32,
    pub(crate) uid: u32,
}

pub(crate) type ProcessRegistry = BTreeMap<u32, ProcessFact>;
pub(crate) type ProcessSnapshot = BTreeMap<u32, ProcessFact>;

pub(crate) fn registry_path(output: &Path) -> PathBuf {
    output.join("processes.jsonl")
}

pub(crate) fn register_children(
    current: &ProcessSnapshot,
    registry: &mut ProcessRegistry,
    uid: u32,
) -> GateResult<Vec<ProcessFact>> {
    for fact in current.values() {
        if fact.uid != uid {
            return Err(process_error(format!(
                "专属 cgroup 中出现 uid {} 的未预期进程 {}，本 job 只允许 uid {uid}",
                fact.uid, fact.pid
            )));
        }
        if let Some(known) = registry.get(&fact.pid)
            && (known.started != fact.started || known.uid != uid)
        {
            return Err(process_error(format!(
                "进程 {} 的创建身份或 uid 已变化，拒绝接受复用 PID",
                fact.pid
            )));
        }
    }

    let before = registry.clone();
    loop {
        let mut progressed = false;
        let candidates = current
            .values()
            .filter(|fact| !registry.contains_key(&fact.pid))
            .cloned()
            .collect::<Vec<_>>();
        for fact in candidates {
            let parent = registry.get(&fact.parent);
            let live_parent = current.get(&fact.parent);
            if parent.zip(live_parent).is_some_and(|(parent, live)| {
                live.started == parent.started && live.uid == parent.uid && parent.uid == uid
            }) {
                registry.insert(fact.pid, fact.clone());
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    Ok(registry
        .iter()
        .filter(|(pid, fact)| before.get(pid) != Some(*fact))
        .map(|(_, fact)| fact.clone())
        .collect())
}

pub(crate) fn verify_processes(
    current: &ProcessSnapshot,
    registry: &ProcessRegistry,
    uid: u32,
) -> GateResult<()> {
    for fact in current.values() {
        let known = registry.get(&fact.pid).ok_or_else(|| {
            process_error(format!(
                "清理发现未登记进程 {}；拒绝终止并保留 cgroup",
                fact.pid
            ))
        })?;
        if fact.uid != uid || fact.started != known.started || fact.uid != known.uid {
            return Err(process_error(format!(
                "清理发现进程 {} 的 uid 或启动身份已变化；拒绝终止并保留 cgroup",
                fact.pid
            )));
        }
    }
    Ok(())
}

pub(crate) fn create_registry(path: &Path, fact: &ProcessFact) -> GateResult<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| GateError::io("process", "创建进程登记", path, &error))?;
    write_fact(&mut file, path, fact)
}

pub(crate) fn append_registry(path: &Path, facts: &[ProcessFact]) -> GateResult<()> {
    if facts.is_empty() {
        return Ok(());
    }
    evidence::reject_symlink(path)?;
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| GateError::io("process", "打开进程登记", path, &error))?;
    for fact in facts {
        write_fact(&mut file, path, fact)?;
    }
    Ok(())
}

pub(crate) fn read_registry(path: &Path) -> GateResult<ProcessRegistry> {
    if evidence::path_absent(path)? {
        return Ok(BTreeMap::new());
    }
    evidence::reject_symlink(path)?;
    let contents = fs::read_to_string(path)
        .map_err(|error| GateError::io("process", "读取进程登记", path, &error))?;
    let mut registry = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fact: ProcessFact = serde_json::from_str(line).map_err(|error| {
            process_error(format!(
                "解析进程登记 {} 第 {} 行失败：{error}",
                path.display(),
                index + 1
            ))
        })?;
        if let Some(previous) = registry.insert(fact.pid, fact.clone())
            && previous.started != fact.started
        {
            return Err(process_error(format!(
                "进程登记中 PID {} 出现多个启动身份，拒绝接受复用 PID",
                fact.pid
            )));
        }
    }
    Ok(registry)
}

fn write_fact(file: &mut std::fs::File, path: &Path, fact: &ProcessFact) -> GateResult<()> {
    serde_json::to_writer(&mut *file, fact)
        .map_err(|error| GateError::new("process", format!("序列化进程登记失败：{error}")))?;
    file.write_all(b"\n")
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_data())
        .map_err(|error| GateError::io("process", "同步进程登记", path, &error))
}

fn process_error(message: impl Into<String>) -> GateError {
    GateError::new("process", message)
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    error::{GateError, GateResult},
    plan::CgroupPlan,
    processes::ProcessFact,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CgroupPhase {
    Prepare,
    Test,
    Cleanup,
}

impl CgroupPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Test => "test",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KernelIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParentState {
    pub(crate) identity: KernelIdentity,
    pub(crate) controllers: Vec<String>,
}

pub(crate) fn parent_state_from(
    mountinfo: &str,
    subtree_control: &str,
    cgroup_root: &Path,
    identity: KernelIdentity,
) -> GateResult<ParentState> {
    let root = cgroup_root.to_string_lossy();
    let mounted = mountinfo.lines().any(|line| {
        let Some((before, after)) = line.split_once(" - ") else {
            return false;
        };
        before.split_whitespace().nth(4) == Some(root.as_ref())
            && after.split_whitespace().next() == Some("cgroup2")
    });
    if !mounted {
        return Err(GateError::new(
            "cgroup",
            format!(
                "CI 必须具有明确挂载的 cgroup v2：/proc/self/mountinfo 未找到 {} 的 cgroup2 挂载",
                cgroup_root.display()
            ),
        ));
    }
    let controllers = subtree_control
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !controllers.iter().any(|controller| controller == "memory") {
        return Err(GateError::new(
            "cgroup",
            format!(
                "父 cgroup {} 未启用 memory controller（当前：{}）；拒绝修改全局 controller 或降级测量，请由宿主预先委托 memory",
                cgroup_root.display(),
                controllers.join(",")
            ),
        ));
    }
    Ok(ParentState {
        identity,
        controllers,
    })
}

pub(crate) fn parse_events(contents: &str) -> GateResult<BTreeMap<String, String>> {
    let mut events = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let mut fields = line.split_whitespace();
        let key = fields.next().ok_or_else(|| {
            GateError::new(
                "cgroup",
                format!("cgroup.events 第 {} 行缺少字段：{line:?}", index + 1),
            )
        })?;
        let value = fields.next().ok_or_else(|| {
            GateError::new(
                "cgroup",
                format!("cgroup.events 第 {} 行缺少值：{line:?}", index + 1),
            )
        })?;
        if fields.next().is_some() {
            return Err(GateError::new(
                "cgroup",
                format!("cgroup.events 第 {} 行包含多余字段：{line:?}", index + 1),
            ));
        }
        if events.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(GateError::new(
                "cgroup",
                format!("cgroup.events 出现重复字段 {key}"),
            ));
        }
    }
    Ok(events)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatedReceipt {
    pub(crate) root: PathBuf,
    pub(crate) run_id: String,
    pub(crate) attempt: String,
    pub(crate) identity: KernelIdentity,
    pub(crate) parent: ParentState,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PrepareResult {
    pub(crate) status: &'static str,
    pub(crate) root: PathBuf,
    pub(crate) identity: KernelIdentity,
    pub(crate) parent: ParentState,
    pub(crate) global_controller_modified: bool,
    pub(crate) limits_modified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TestResult {
    pub(crate) status: &'static str,
    pub(crate) case: &'static str,
    pub(crate) exit_code: i32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) cargo_log_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupStatus {
    NotCreated,
    Cleaned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CleanupResult {
    pub(crate) status: CleanupStatus,
    pub(crate) root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity: Option<KernelIdentity>,
    #[serde(default)]
    pub(crate) terminated_registered_processes: Vec<ProcessFact>,
    #[serde(default)]
    pub(crate) root_absent: bool,
}

impl CleanupResult {
    pub(crate) fn not_created(root: PathBuf) -> Self {
        Self {
            status: CleanupStatus::NotCreated,
            root,
            identity: None,
            terminated_registered_processes: Vec::new(),
            root_absent: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhaseReceipt<T> {
    run_id: String,
    attempt: String,
    #[serde(flatten)]
    result: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CleanupPreflight {
    NotCreated,
    Reuse,
    Proceed,
}

pub(crate) fn inspect_cleanup(plan: &CgroupPlan, output: &Path) -> GateResult<CleanupPreflight> {
    let cleanup_path = output.join("cleanup.json");
    if !path_absent(&cleanup_path)? {
        let previous: PhaseReceipt<CleanupResult> = read_json(&cleanup_path)?;
        if previous.run_id != plan.run_id
            || previous.attempt != plan.attempt
            || previous.result.root != plan.root
        {
            return Err(evidence_error(
                "清理收据的 run、attempt 或 root 与当前计划不一致",
            ));
        }
        if !path_absent(&plan.root)? {
            return Err(evidence_error(format!(
                "清理收据声称已结束，但精确 cgroup {} 仍存在",
                plan.root.display()
            )));
        }
        match previous.result.status {
            CleanupStatus::NotCreated => {
                if !path_absent(&output.join("created.json"))? {
                    return Err(evidence_error(
                        "未创建收据与后来出现的 created.json 冲突，拒绝覆盖验收事实",
                    ));
                }
                Ok(CleanupPreflight::Reuse)
            }
            CleanupStatus::Cleaned => {
                let created: CreatedReceipt = read_json(&output.join("created.json"))?;
                if previous.result.identity.as_ref() != Some(&created.identity)
                    || created.root != plan.root
                    || created.run_id != plan.run_id
                    || created.attempt != plan.attempt
                {
                    return Err(evidence_error("清理收据与本次创建证据的内核身份不一致"));
                }
                Ok(CleanupPreflight::Reuse)
            }
        }
    } else {
        let created_path = output.join("created.json");
        if path_absent(&created_path)? {
            if !path_absent(&plan.root)? {
                return Err(evidence_error(format!(
                    "没有本次创建证据，拒绝清理已有 cgroup {}",
                    plan.root.display()
                )));
            }
            return Ok(CleanupPreflight::NotCreated);
        }
        Ok(CleanupPreflight::Proceed)
    }
}

pub(crate) fn read_created(output: &Path) -> GateResult<CreatedReceipt> {
    read_json(&output.join("created.json"))
}

pub(crate) fn read_cleanup_result(output: &Path) -> GateResult<CleanupResult> {
    let receipt: PhaseReceipt<CleanupResult> = read_json(&output.join("cleanup.json"))?;
    Ok(receipt.result)
}

pub(crate) fn write_phase_receipt<T: Serialize>(
    phase: CgroupPhase,
    plan: &CgroupPlan,
    output: &Path,
    result: &T,
) -> GateResult<()> {
    write_json_new(
        &output.join(format!("{}.json", phase.as_str())),
        &PhaseReceipt {
            run_id: plan.run_id.clone(),
            attempt: plan.attempt.clone(),
            result,
        },
    )
}

pub(crate) fn write_failure(
    phase: CgroupPhase,
    output: &Path,
    plan: Option<&CgroupPlan>,
    error: &GateError,
) -> GateResult<PathBuf> {
    let path = output.join(format!(
        "{}-failure-{}.json",
        phase.as_str(),
        uuid::Uuid::new_v4().simple()
    ));
    write_json_new(
        &path,
        &serde_json::json!({
            "run_id": plan.map(|plan| plan.run_id.as_str()),
            "attempt": plan.map(|plan| plan.attempt.as_str()),
            "root": plan.map(|plan| plan.root.to_string_lossy().into_owned()),
            "error_type": error.kind(),
            "error": error.to_string(),
            "status": "failed",
        }),
    )?;
    Ok(path)
}

pub(crate) fn write_runner_failure(output: &Path, error: &GateError) -> GateResult<PathBuf> {
    let path = output.join(format!(
        "runner-failure-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    write_json_new(
        &path,
        &serde_json::json!({
            "error_type": error.kind(),
            "error": error.to_string(),
            "status": "failed",
        }),
    )?;
    Ok(path)
}

pub(crate) fn write_json_new<T: Serialize>(path: &Path, value: &T) -> GateResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| GateError::new("evidence", format!("序列化 cgroup 证据失败：{error}")))?;
    bytes.push(b'\n');
    write_bytes_new(path, &bytes)
}

fn write_bytes_new(path: &Path, value: &[u8]) -> GateResult<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| GateError::io("evidence", "创建只写验收文件", path, &error))?;
    file.write_all(value)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| GateError::io("evidence", "写入验收文件", path, &error))
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> GateResult<T> {
    reject_symlink(path)?;
    let bytes =
        fs::read(path).map_err(|error| GateError::io("evidence", "读取验收证据", path, &error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        GateError::new("evidence", format!("解析 {} 失败：{error}", path.display()))
    })
}

pub(crate) fn reject_symlink(path: &Path) -> GateResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(GateError::new(
            "evidence",
            format!("拒绝把符号链接当作 cgroup 证据：{}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GateError::io("evidence", "核验验收路径", path, &error)),
    }
}

pub(crate) fn path_absent(path: &Path) -> GateResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(GateError::io("evidence", "核验路径状态", path, &error)),
    }
}

fn evidence_error(message: impl Into<String>) -> GateError {
    GateError::new("evidence", message)
}

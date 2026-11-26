//! 仅观察原 API 监督失败，不签发正常关闭证明。
use crate::Result;
use serde_json::Value;
use std::path::Path;

#[cfg(any(windows, test))]
use super::super::documents;
#[cfg(any(windows, test))]
use serde_json::json;

#[cfg(windows)]
pub(crate) fn observe_windows_api_incident(
    directory: &Path,
    runtime: &Value,
    api_url: &str,
    worker_url: &str,
) -> Result<Value> {
    use super::super::{operation, role::Role, tree};
    use crate::recovery::{
        clone::native::{resources::common, target_verify::authority::require_closed},
        monitoring::execution::ProtectedInputs,
    };

    let directory = operation::canonical(directory)?;
    let role = Role::Api;
    let registration = tree::read(&directory, runtime, role, true)?.ok_or("缺少原 API 进程树")?;
    let id = registration["operation_id"]
        .as_str()
        .ok_or("缺少原 API 操作ID")?;
    let api = directory.join("api.json");
    let original_tree = directory.join("api-tree.json");
    let ready = tree::path(&directory, role, id, "ready");
    let failure = tree::path(&directory, role, id, "stopped");
    let directory_id =
        common::device_inode_for(&directory, &std::fs::symlink_metadata(&directory)?)?;
    let protected = ProtectedInputs::open(
        [&api, &original_tree, &ready, &failure].map(|path| path.to_path_buf()),
    )?;
    protected.finish((|| {
        if tree::read(&directory, runtime, role, true)?.as_ref() != Some(&registration)
            || operation::read(&api)?
                != json!({"format_version":1,"role":"api",
                "scope_id":registration["scope_id"],"identity":registration["process"]})
            || registration["process"]["executable"] != runtime["artifacts"]["api"]["path"]
        {
            return Err("原 API 进程登记与运行产物不匹配".into());
        }
        let api_root = api_url.strip_suffix("/readyz").unwrap_or(api_url);
        let worker_root = worker_url
            .strip_suffix("/readyz")
            .ok_or("原 Worker 就绪端点无效")?;
        if common::endpoint(api_root)?.0 == common::endpoint(worker_root)?.0
            || runtime["worker_ready_url"] != worker_url
            || runtime
                .get("api_ready_url")
                .is_some_and(|registered| *registered != format!("{api_root}/readyz"))
        {
            return Err("事故监听端点与原运行登记不匹配".into());
        }
        let failed = operation::read(&failure)?;
        let members = failed_members(&registration, &operation::read(&ready)?, &failed)?;
        let monitor = &registration["monitor"];
        let before = same_boot(members, monitor)?;
        for _ in 0..2 {
            require_unowned(&directory, id)?;
            for member in members.iter().chain(std::iter::once(monitor)) {
                require_retired(member)?;
            }
            require_closed(api_root)?;
            require_closed(worker_url)?;
        }
        require_unowned(&directory, id)?;
        if before.abs_diff(same_boot(members, monitor)?) > BOOT_DRIFT_TICKS
            || common::device_inode_for(&directory, &std::fs::symlink_metadata(&directory)?)?
                != directory_id
        {
            return Err("原运行目录或 Windows 启动刻度在事故观察期间变化".into());
        }
        Ok(json!({
            "kind":"windows-api-incomplete-cleanup-incident",
            "scope_id":registration["scope_id"], "operation_id":registration["operation_id"],
            "api":common::binding(&api)?, "tree":common::binding(&original_tree)?,
            "ready":common::binding(&ready)?, "failure":common::binding(&failure)?,
            "identities":{"monitor":monitor, "supervisor":registration["supervisor"],
                "process":registration["process"], "members":members},
            "normal_job_exit_proof":null
        }))
    })())
}

#[cfg(not(windows))]
pub(crate) fn observe_windows_api_incident(_: &Path, _: &Value, _: &str, _: &str) -> Result<Value> {
    Err("API 监督事故观察仅支持 Windows".into())
}

#[cfg(windows)]
const BOOT_DRIFT_TICKS: u64 = 60 * 10_000_000;

#[cfg(windows)]
fn require_unowned(directory: &Path, id: &str) -> Result<()> {
    let external = format!("api-external-termination-{id}.json");
    let control = format!("api-tree-{id}-control.json");
    for entry in std::fs::read_dir(directory)? {
        let name = entry?.file_name().to_string_lossy().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "api-control.lock" | "worker.json" | "worker.log"
        ) || name.starts_with("worker-")
            || name == external
            || name == control
        {
            return Err("原 API 事故目录有其他控制、关闭或 Worker 进程证据".into());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn same_boot(members: &[Value], monitor: &Value) -> Result<u64> {
    use crate::recovery::source::evidence::{host_boot_proof, verify_host_boot_proof};

    let proof = host_boot_proof()?;
    let boot = verify_host_boot_proof(&proof)?;
    let lower = boot
        .checked_add(BOOT_DRIFT_TICKS)
        .ok_or("Windows 启动刻度溢出")?;
    let upper = proof["observed_ticks"]
        .as_u64()
        .ok_or("缺少 Windows 采集刻度")?;
    for member in members.iter().chain(std::iter::once(monitor)) {
        let born = started(member)?;
        if born <= lower || born > upper {
            return Err("原 API 观察成员没有处于本次 Windows 启动之后".into());
        }
    }
    Ok(boot)
}

#[cfg(windows)]
fn require_retired(expected: &Value) -> Result<()> {
    use crate::recovery::clone::native::lifecycle::identity;
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

    let pid = u32::try_from(expected["pid"].as_u64().ok_or("原 API 成员PID无效")?)?;
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(()),
        Err(error) => return Err(format!("无法核对原 API 进程 {pid}：{error}").into()),
    };
    let signaled =
        || -> Result<bool> { Ok(handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0) };
    if !signaled()? {
        return Err("原 API 事故成员使用的 PID 仍有存活进程，保留现场".into());
    }
    let (creation, exit) = identity::windows_lifetime(&handle, pid)?;
    let original = started(expected)?;
    if !closed_lifetime(original, creation, exit) {
        return Err("可读进程句柄的创建或退出刻度无法证明历史成员关闭".into());
    }
    // 同代须吻合原映像。较晚创建的进程已实际复用了该 PID，仍须由同一持有句柄证明退出。
    if creation == original && identity::windows_snapshot(&handle, pid)? != *expected {
        return Err("已退出的同代进程与原 API 成员身份不同".into());
    }
    if !signaled()? {
        return Err("核对身份期间丢失同一进程句柄的退出证明".into());
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn closed_lifetime(original: u64, creation: u64, exit: u64) -> bool {
    original > 0 && creation >= original && exit >= creation
}

#[cfg(any(windows, test))]
fn started(identity: &Value) -> Result<u64> {
    documents::identity(identity)?;
    u32::try_from(identity["pid"].as_u64().ok_or("原 API 成员PID无效")?)?;
    let value = identity["started"]
        .as_str()
        .ok_or("原 API 成员创建刻度无效")?
        .parse::<u64>()?;
    if value == 0 {
        return Err("原 API 成员创建刻度不能为零".into());
    }
    Ok(value)
}

#[cfg(any(windows, test))]
fn failed_members<'a>(tree: &Value, ready: &Value, failed: &'a Value) -> Result<&'a [Value]> {
    use std::collections::BTreeSet;

    let (monitor, supervisor, process) = (&tree["monitor"], &tree["supervisor"], &tree["process"]);
    let supervisor_start = started(supervisor)?;
    let mut owner = json!({"format_version":1,"operation_id":tree["operation_id"],
        "directory":tree["runtime_directory"],"role":"api","scope_id":tree["scope_id"],
        "supervisor":supervisor,"monitor":monitor});
    if *ready != owner
        || supervisor["pid"] == process["pid"]
        || monitor["pid"] == supervisor["pid"]
        || monitor["pid"] == process["pid"]
        || monitor["executable"] != supervisor["executable"]
        || started(monitor)? >= supervisor_start
        || supervisor_start >= started(process)?
    {
        return Err("事故监督与产品创建身份不属于原 API 启动".into());
    }
    owner["status"] = json!("failed");
    owner["error_type"] = json!("MemberCleanupFailed");
    owner["members"] = failed["members"].clone();
    let members = failed["members"]
        .as_array()
        .ok_or("原 API 成员失败记录缺少成员")?;
    if *failed != owner || !members.contains(supervisor) || !members.contains(process) {
        return Err("原 API 成员失败记录状态无效或缺少已登记成员".into());
    }
    let mut observed = BTreeSet::new();
    for member in members {
        let born = started(member)?;
        if member["pid"] == monitor["pid"]
            || born < supervisor_start
            || !observed.insert((member["pid"].as_u64(), born))
        {
            return Err("原 API 事故含 Job 外监督、过早或重复的成员身份".into());
        }
    }
    Ok(members)
}

#[cfg(test)]
#[test]
fn api_incident_rejects_older_reuse_or_missing_exit_time() {
    assert!(closed_lifetime(10, 10, 12));
    assert!(closed_lifetime(10, 15, 18));
    assert!(!closed_lifetime(10, 9, 18));
    assert!(!closed_lifetime(10, 15, 0));
    assert!(!closed_lifetime(10, 15, 14));
}

#[cfg(test)]
#[test]
fn api_incident_requires_exact_failed_scope_and_original_members() {
    let executable = std::env::current_exe().unwrap();
    let monitor = json!({"executable":executable,"pid":100,"started":"10"});
    let supervisor = json!({"executable":executable,"pid":101,"started":"20"});
    let product = json!({"executable":executable,"pid":102,"started":"30"});
    let mut tree = json!({"runtime_directory":executable.parent(),"operation_id":"operation",
        "scope_id":"original","process":product,"supervisor":supervisor,"monitor":monitor});
    let ready = json!({"format_version":1,"operation_id":"operation","role":"api","scope_id":"original",
        "directory":executable.parent(),"supervisor":supervisor,"monitor":monitor});
    let mut failed = ready.clone();
    failed["status"] = json!("failed");
    failed["error_type"] = json!("MemberCleanupFailed");
    failed["members"] = json!([supervisor, product]);
    assert_eq!(failed_members(&tree, &ready, &failed).unwrap().len(), 2);

    let mut wrong_scope = failed.clone();
    wrong_scope["scope_id"] = json!("other");
    assert!(failed_members(&tree, &ready, &wrong_scope).is_err());
    let mut wrong_ready = ready.clone();
    wrong_ready["operation_id"] = json!("other");
    assert!(failed_members(&tree, &wrong_ready, &failed).is_err());
    let mut missing = failed.clone();
    missing["members"] = json!([supervisor]);
    assert!(failed_members(&tree, &ready, &missing).is_err());
    let mut success = failed.clone();
    success["status"] = json!("stopped");
    success["error_type"] = Value::Null;
    assert!(failed_members(&tree, &ready, &success).is_err());
    tree["process"]["pid"] = supervisor["pid"].clone();
    assert!(failed_members(&tree, &ready, &failed).is_err());
}

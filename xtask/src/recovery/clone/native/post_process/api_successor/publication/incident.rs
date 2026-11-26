//! 核对第二代接续绑定的原发布、监督失败和正式启动请求。

use super::{Verified, read_at};
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, target_verify::guard::Inputs},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) fn verify_second(
    backend: &Path,
    restart: u64,
    intent: &Value,
    previous: &Verified,
) -> Result<()> {
    if restart <= previous.selected.passed_attempt
        || intent["predecessor"] != previous.result
        || intent["marker"] != previous.intent["marker"]
    {
        return Err("第二代 API 启动未保留原发布及 Redis 标记绑定".into());
    }
    let request = verify_request(backend, &intent["request"], &previous.result)?;
    let publication = &previous.selected.publication;
    let directory = Path::new(document::text(
        &publication["runtime_directory"],
        "前代运行目录",
    )?);
    let incident = &intent["incident"];
    document::exact(
        incident,
        &[
            "kind",
            "scope_id",
            "operation_id",
            "api",
            "tree",
            "ready",
            "failure",
            "identities",
            "normal_job_exit_proof",
        ],
        "第二代 API 事故",
    )?;
    let operation = super::super::token(&previous.intent["token"])?;
    if incident["kind"] != "windows-api-incomplete-cleanup-incident"
        || incident["normal_job_exit_proof"] != Value::Null
        || incident["operation_id"] != operation
        || incident["api"] != publication["api"]
        || incident["tree"] != publication["api_tree"]
    {
        return Err("第二代 API 事故不属于原发布的同一进程树".into());
    }
    let bindings = [
        &previous.result,
        &previous.selected.binding,
        &publication["intent"],
        &intent["marker"],
        &intent["request"],
        &request["failed_verification"],
        &incident["api"],
        &incident["tree"],
        &incident["ready"],
        &incident["failure"],
    ];
    let protected = Inputs::open(
        bindings
            .iter()
            .map(|binding| common::bound_file(backend, binding, "第二代 API 事故来源"))
            .collect::<Result<Vec<_>>>()?,
    )?;
    if common::read_bound(backend, &intent["request"], "第二代 API 启动请求")? != request {
        return Err("第二代 API 启动请求在保护前变化".into());
    }
    let api = read_at(backend, &incident["api"], &directory.join("api.json"))?;
    let tree = read_at(backend, &incident["tree"], &directory.join("api-tree.json"))?;
    let ready = read_at(
        backend,
        &incident["ready"],
        &directory.join(format!("api-members-{operation}-ready.json")),
    )?;
    let failure = read_at(
        backend,
        &incident["failure"],
        &directory.join(format!("api-members-{operation}-stopped.json")),
    )?;
    verify_members(incident, &api, &tree, &ready, &failure)?;
    common::bound_file(backend, &request["failed_verification"], "原核验失败证据")?;
    protected.check()
}

fn verify_request(backend: &Path, binding: &Value, previous: &Value) -> Result<Value> {
    let request = common::read_bound(backend, binding, "第二代 API 启动请求")?;
    document::exact(
        &request,
        &[
            "format_version",
            "kind",
            "predecessor",
            "failed_verification",
        ],
        "第二代 API 启动请求",
    )?;
    if request["format_version"] != 1
        || request["kind"] != "devex-clone-api-successor-request"
        || request["predecessor"] != *previous
    {
        return Err("第二代 API 启动请求不属于同一原发布".into());
    }
    Ok(request)
}

fn verify_members(
    incident: &Value,
    api: &Value,
    tree: &Value,
    ready: &Value,
    failure: &Value,
) -> Result<()> {
    let (monitor, supervisor, process) = (&tree["monitor"], &tree["supervisor"], &tree["process"]);
    let identities = &incident["identities"];
    let members = &identities["members"];
    let expected_ready = json!({
        "format_version":1,"operation_id":tree["operation_id"],"directory":tree["runtime_directory"],
        "role":"api","scope_id":tree["scope_id"],"supervisor":supervisor,"monitor":monitor,
    });
    let mut expected_failure = expected_ready.clone();
    expected_failure["status"] = json!("failed");
    expected_failure["error_type"] = json!("MemberCleanupFailed");
    expected_failure["members"] = members.clone();
    if incident["scope_id"] != tree["scope_id"]
        || incident["operation_id"] != tree["operation_id"]
        || api["identity"] != *process
        || *identities
            != json!({"monitor":monitor,"supervisor":supervisor,"process":process,"members":members})
        || *ready != expected_ready
        || *failure != expected_failure
    {
        return Err("第二代 API 事故的成员、就绪或失败停止记录与先前发布不符".into());
    }

    let (monitor_pid, monitor_start) = member_key(monitor)?;
    let (supervisor_pid, supervisor_start) = member_key(supervisor)?;
    let (process_pid, process_start) = member_key(process)?;
    let members = document::array(members, "原 API 事故成员")?;
    if monitor_pid == supervisor_pid
        || monitor_pid == process_pid
        || supervisor_pid == process_pid
        || monitor["executable"] != supervisor["executable"]
        || monitor_start >= supervisor_start
        || supervisor_start >= process_start
        || !members.contains(supervisor)
        || !members.contains(process)
    {
        return Err("第二代 API 事故缺少原监督、产品或准确的创建顺序".into());
    }
    let mut observed = BTreeSet::new();
    for member in members {
        let (pid, started) = member_key(member)?;
        if pid == monitor_pid || started < supervisor_start || !observed.insert((pid, started)) {
            return Err("第二代 API 事故含重复、过早或 Job 外进程".into());
        }
    }
    Ok(())
}

fn member_key(value: &Value) -> Result<(u32, u64)> {
    common::validate_process_identity(value)?;
    let pid = u32::try_from(value["pid"].as_u64().ok_or("事故成员 PID 无效")?)?;
    let started = document::text(&value["started"], "事故成员创建刻度")?.parse::<u64>()?;
    if started == 0 {
        return Err("事故成员创建刻度不能为零".into());
    }
    Ok((pid, started))
}

#[cfg(test)]
#[test]
fn api_successor_incident_requires_failed_stop_and_original_members() -> Result<()> {
    let directory = if cfg!(windows) {
        Path::new(r"D:\fixture")
    } else {
        Path::new("/fixture")
    };
    let controller = directory.join("xtask.exe");
    let product = directory.join("ryframe-api.exe");
    let monitor = json!({"pid":21,"started":"100","executable":controller});
    let supervisor = json!({"pid":22,"started":"200","executable":controller});
    let process = json!({"pid":23,"started":"300","executable":product});
    let tree = json!({"runtime_directory":directory,"operation_id":"operation","scope_id":"scope",
        "monitor":monitor,"supervisor":supervisor,"process":process});
    let ready = json!({"format_version":1,"directory":directory,"operation_id":"operation",
        "role":"api","scope_id":"scope","monitor":monitor,"supervisor":supervisor});
    let mut incident = json!({"operation_id":"operation","scope_id":"scope",
        "identities":{"monitor":monitor,"supervisor":supervisor,"process":process,
        "members":[supervisor,process]}});
    let api = json!({"identity":process});
    let mut failure = ready.clone();
    failure["status"] = json!("failed");
    failure["error_type"] = json!("MemberCleanupFailed");
    failure["members"] = incident["identities"]["members"].clone();
    verify_members(&incident, &api, &tree, &ready, &failure)?;
    failure["status"] = json!("stopped");
    assert!(verify_members(&incident, &api, &tree, &ready, &failure).is_err());
    failure["status"] = json!("failed");
    failure["members"] = json!([process]);
    incident["identities"]["members"] = failure["members"].clone();
    assert!(verify_members(&incident, &api, &tree, &ready, &failure).is_err());
    Ok(())
}

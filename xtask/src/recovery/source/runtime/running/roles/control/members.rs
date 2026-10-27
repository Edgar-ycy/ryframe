use super::*;

pub(super) fn verify(
    runtime: &Path,
    role: &str,
    id: &str,
    records: &serde_json::Map<String, Value>,
    archived: bool,
) -> Result<()> {
    let bindings = snapshot(runtime, role, id)?;
    let request = &records["request.json"];
    let tree_path = runtime.join(format!("{role}-tree.json"));
    let ready = ready(runtime, role, id, records)?;
    stopped(runtime, role, id, records, &ready, archived)?;
    if tree_path.try_exists()? {
        let tree = super::super::tree(runtime, role, &request["scope_id"])?;
        let identity = super::super::process(runtime, role, &request["scope_id"])?;
        if tree["operation_id"] != id
            || tree["process"] != identity
            || ready["supervisor"] != tree["supervisor"]
            || ready["monitor"] != tree["monitor"]
            || identity["executable"] != request[format!("{role}_artifact")]["path"]
            || records
                .get("progress.json")
                .is_some_and(|v| !v["identity"].is_null() && v["identity"] != identity)
            || records
                .get("result.json")
                .is_some_and(|v| !v["identity"].is_null() && v["identity"] != identity)
        {
            return Err("源实际进程树、产物与控制链身份不一致".into());
        }
        tree_receipts(runtime, role, id, &tree)?;
    } else if runtime.join(format!("{role}.json")).try_exists()?
        || (archived && records["result.json"]["outcome"] == "succeeded")
    {
        return Err("源角色进程或成功归档缺少完整监督树".into());
    }
    for (path, binding) in bindings {
        if common::binding(&path)? != binding {
            return Err("源角色成员或树收据在核验期间变化".into());
        }
    }
    Ok(())
}

fn snapshot(runtime: &Path, role: &str, id: &str) -> Result<Vec<(std::path::PathBuf, Value)>> {
    let names = [
        format!("{role}.json"),
        format!("{role}-tree.json"),
        format!("{role}-members-{id}-ready.json"),
        format!("{role}-members-{id}-stopped.json"),
        format!("{role}-tree-{id}-control.json"),
        format!("{role}-tree-{id}-result.json"),
    ];
    let mut files = Vec::new();
    for name in names {
        let path = runtime.join(name);
        if path.try_exists()? {
            plain(&path, false)?;
            files.push((path.clone(), common::binding(&path)?));
        }
    }
    Ok(files)
}

fn ready(
    runtime: &Path,
    role: &str,
    id: &str,
    records: &serde_json::Map<String, Value>,
) -> Result<Value> {
    let request = &records["request.json"];
    let ready_path = runtime.join(format!("{role}-members-{id}-ready.json"));
    let tree_path = runtime.join(format!("{role}-tree.json"));
    let mut ready = Value::Null;
    if ready_path.try_exists()? {
        ready = read(&ready_path)?;
        document::exact(
            &ready,
            &[
                "format_version",
                "operation_id",
                "directory",
                "role",
                "scope_id",
                "supervisor",
                "monitor",
            ],
            "源角色成员启动证明",
        )?;
        if ready["format_version"] != 1
            || ready["operation_id"] != id
            || ready["directory"] != json!(runtime)
            || ready["role"] != role
            || ready["scope_id"] != request["scope_id"]
        {
            return Err("源角色成员证明不属于原请求".into());
        }
        common::validate_process_identity(&ready["supervisor"])?;
        common::validate_process_identity(&ready["monitor"])?;
        if records
            .get("candidate.json")
            .is_some_and(|v| v["identity"] != ready["supervisor"])
            || records
                .get("supervisor.json")
                .is_some_and(|v| v["identity"] != ready["supervisor"])
        {
            return Err("源成员监督器没有绑定控制链候选与授权".into());
        }
    } else if records.contains_key("candidate.json")
        || records.contains_key("supervisor.json")
        || tree_path.try_exists()?
    {
        return Err("源角色授权或进程树缺少原成员启动证明".into());
    }
    Ok(ready)
}

fn stopped(
    runtime: &Path,
    role: &str,
    id: &str,
    records: &serde_json::Map<String, Value>,
    ready: &Value,
    archived: bool,
) -> Result<()> {
    let stopped_path = runtime.join(format!("{role}-members-{id}-stopped.json"));
    if stopped_path.try_exists()? {
        let stopped = read(&stopped_path)?;
        document::exact(
            &stopped,
            &[
                "format_version",
                "operation_id",
                "directory",
                "role",
                "scope_id",
                "supervisor",
                "monitor",
                "status",
                "members",
                "error_type",
            ],
            "源角色成员结果",
        )?;
        if ready.is_null()
            || [
                "format_version",
                "operation_id",
                "directory",
                "role",
                "scope_id",
                "supervisor",
                "monitor",
            ]
            .iter()
            .any(|key| stopped[*key] != ready[*key])
            || !matches!(stopped["status"].as_str(), Some("stopped" | "failed"))
        {
            return Err("源成员完成证明与原启动不同".into());
        }
        let members = document::array(&stopped["members"], "源完整成员集合")?;
        if !members.contains(&ready["supervisor"])
            || (stopped["status"] == "stopped") != stopped["error_type"].is_null()
        {
            return Err("源成员完成证明缺少监督者或错误状态不一致".into());
        }
        for member in members {
            common::validate_process_identity(member)?;
        }
    } else if archived && records["result.json"]["outcome"] == "failed" {
        return Err("源失败角色归档缺少原监督器完整成员收尾".into());
    }
    Ok(())
}

fn tree_receipts(runtime: &Path, role: &str, id: &str, tree: &Value) -> Result<()> {
    let control = runtime.join(format!("{role}-tree-{id}-control.json"));
    if control.try_exists()? {
        let value = read(&control)?;
        document::exact(
            &value,
            &[
                "format_version",
                "kind",
                "operation_id",
                "mode",
                "requested_at_ns",
            ],
            "源树停止请求",
        )?;
        if value["format_version"] != 1
            || value["kind"] != "full-stack-process-tree-control"
            || value["operation_id"] != id
            || !matches!(value["mode"].as_str(), Some("normal" | "crash"))
            || !value["requested_at_ns"].as_u64().is_some_and(|n| n > 0)
        {
            return Err("源树停止请求不属于同一启动意图".into());
        }
    }
    let result = runtime.join(format!("{role}-tree-{id}-result.json"));
    if result.try_exists()? {
        let value = read(&result)?;
        document::exact(
            &value,
            &[
                "format_version",
                "kind",
                "operation_id",
                "process",
                "exit_code",
                "termination",
            ],
            "源树退出结果",
        )?;
        if value["format_version"] != 1
            || value["kind"] != "full-stack-process-tree-result"
            || value["operation_id"] != id
            || value["process"] != tree["process"]
            || !value["exit_code"]
                .as_i64()
                .is_some_and(|n| i32::try_from(n).is_ok())
            || !matches!(
                value["termination"].as_str(),
                Some("natural" | "normal" | "forced")
            )
        {
            return Err("源树退出结果没有绑定原产品身份".into());
        }
    }
    Ok(())
}

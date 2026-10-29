use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use super::{generation, prefix};
#[path = "close_recovery.rs"]
mod close_recovery;
use crate::{
    Result,
    recovery::clone::native::{digest, json as json_io, lifecycle::state},
};
pub(crate) use close_recovery::{candidate as close_candidate, recovery_evidence};

#[derive(Debug)]
pub(super) struct HistoryError(pub(super) &'static str);
impl std::fmt::Display for HistoryError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(self.0)
    }
}
impl std::error::Error for HistoryError {}

pub(super) fn check(valid: bool, message: &'static str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Box::new(HistoryError(message)))
    }
}

pub(in crate::recovery::fixture_control::services) fn exact(
    value: &Value,
    fields: &[&str],
) -> Result<()> {
    json_io::exact(value, fields, "夹具历史证据")
        .map(|_| ())
        .map_err(|_| {
            Box::new(HistoryError("夹具历史证据字段不完整或存在未知字段"))
                as Box<dyn std::error::Error>
        })
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("夹具历史证据缺少字符串字段 {key}").into())
}

pub(in crate::recovery::fixture_control::services) fn path_is(value: &Value, path: &Path) -> bool {
    value.as_str().is_some_and(|value| Path::new(value) == path)
}

pub(super) fn read(path: &Path) -> Result<Value> {
    check(
        !digest::linked_path(path)?.contains(&true),
        "夹具历史证据不能经过链接",
    )?;
    json_io::read(path)
}

pub(super) fn binding(path: &Path) -> Result<Value> {
    check(
        !digest::linked_path(path)?.contains(&true),
        "夹具历史证据不能经过链接",
    )?;
    super::super::super::file_binding(path, "夹具历史证据")
}

pub(in crate::recovery::fixture_control::services) fn bound(
    run: &Path,
    descriptor: &Value,
) -> Result<Value> {
    exact(descriptor, &["path", "bytes", "sha256"])?;
    let path = Path::new(text(descriptor, "path")?);
    check(
        path.starts_with(run)
            && path != run
            && !path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }),
        "夹具历史证据越过运行目录",
    )?;
    let before = binding(path)?;
    check(&before == descriptor, "夹具历史证据发生变化")?;
    let value = read(path)?;
    check(binding(path)? == before, "夹具历史证据读取期间变化")?;
    Ok(value)
}

pub(crate) fn controller(run: &Path, attempt: &Value) -> Result<Value> {
    let mut running = attempt.clone();
    running["finished_at"] = Value::Null;
    running["status"] = json!("running");
    running["result"] = Value::Null;
    running["error_type"] = Value::Null;
    let number = attempt["number"].as_u64().ok_or("失败阶段编号无效")?;
    let before = binding(&state::controller_path(run, number))?;
    let (descriptor, _) = state::controller_record(run, number, &running).map_err(|_| {
        Box::new(HistoryError("持久控制器与当前阶段完整记录不同")) as Box<dyn std::error::Error>
    })?;
    check(
        descriptor == before && binding(&state::controller_path(run, number))? == before,
        "持久控制器在核验期间变化",
    )?;
    Ok(descriptor)
}

pub(crate) fn restart_evidence(
    run: &Path,
    failed: &Value,
    sources: &Value,
    predecessor: &Value,
) -> Result<Value> {
    let mut expected = sources.as_object().ok_or("失败重启来源无效")?.clone();
    expected.insert("restart_predecessor".into(), predecessor.clone());
    let actual = failed["sources"].as_object().ok_or("失败重启来源无效")?;
    check(
        actual.keys().eq(expected.keys())
            && expected
                .iter()
                .all(|(key, value)| key == "state_before" || actual.get(key) == Some(value))
            && prefix::count(run, &failed["sources"]["state_before"])? + 1
                == failed["number"].as_u64().ok_or("失败阶段编号无效")? as usize,
        "失败 restart 未绑定当前夹具输入及完整账本前像",
    )?;
    Ok(json!({"failed_controller":controller(run, failed)?}))
}

pub(crate) fn close_evidence(
    run: &Path,
    failed: &Value,
    generation: &Value,
    sources: &Value,
    services: &Value,
) -> Result<(Value, Value)> {
    let expected = sources.as_object().ok_or("失败关闭来源无效")?;
    let actual = failed["sources"].as_object().ok_or("失败关闭来源无效")?;
    check(
        expected.keys().eq(actual.keys())
            && expected
                .iter()
                .all(|(key, value)| key == "state_before" || actual.get(key) == Some(value))
            && prefix::count(run, &failed["sources"]["state_before"])? + 1
                == failed["number"].as_u64().ok_or("失败阶段编号无效")? as usize,
        "失败 close 未绑定当前夹具输入及完整账本前像",
    )?;
    let number = failed["number"].as_u64().ok_or("失败阶段编号无效")?;
    let root = run.join(format!("lifecycle-{number:04}"));
    let children = std::fs::read_dir(&root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let names = children
        .iter()
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
        })
        .collect::<BTreeSet<_>>();
    check(
        names == BTreeSet::from(["stopped.json"])
            || names == BTreeSet::from(["stopped.json", "rustfs-stopped.json"]),
        "失败 close 现场包含缺失或未知证据",
    )?;
    for path in &children {
        binding(path)?;
    }
    let redis_path = root.join("stopped.json");
    let redis = read(&redis_path)?;
    check(
        redis["status"] == "redis_process_stopped"
            && redis["alive"] == false
            && redis["resources_deleted"] == false
            && redis["runtime"] == services["runtime"],
        "Redis 失败 close 证据与原服务代次不一致",
    )?;
    let (rustfs, _) = generation::paths(run, generation)?;
    let tree_path = rustfs.join("rustfs-tree.json");
    let tree = read(&tree_path)?;
    check(
        tree == services["tree"] && services["evidence"]["rustfs_tree"] == binding(&tree_path)?,
        "RustFS 失败 close 进程树与原服务代次不一致",
    )?;
    let completion = binding(&rustfs.join(format!(
        "rustfs-members-{}-stopped.json",
        text(&tree, "operation_id")?
    )))?;
    members(
        &json!({"state":"stopped","tree":tree,"completion":completion}),
        run,
    )?;
    let mut evidence = json!({"redis":binding(&redis_path)?,"rustfs_tree":binding(&tree_path)?,"rustfs_members":completion});
    let wrapped = root.join("rustfs-stopped.json");
    if wrapped.exists() {
        let proof = read(&wrapped)?;
        exact(&proof, &["tree", "terminated", "state", "completion"])?;
        check(
            proof["tree"] == tree
                && proof["terminated"].is_boolean()
                && proof["state"] == "stopped"
                && proof["completion"] == completion,
            "RustFS 失败 close 封装证据不完整",
        )?;
        evidence["rustfs_close"] = binding(&wrapped)?;
    }
    Ok((evidence, controller(run, failed)?))
}

pub(in crate::recovery::fixture_control::services) fn members(
    proof: &Value,
    run: &Path,
) -> Result<()> {
    let tree = &proof["tree"];
    check(
        proof["state"] == "stopped" && tree["format_version"] == 2,
        "RustFS 关闭收据未绑定当前进程树",
    )?;
    let directory = Path::new(text(tree, "runtime_directory")?);
    let initial = directory == run.join("rustfs");
    let restarted = directory.file_name().and_then(|name| name.to_str()) == Some("rustfs")
        && directory.parent().and_then(Path::parent) == Some(run)
        && directory
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("lifecycle-"));
    check(
        directory.is_absolute() && directory.starts_with(run) && (initial || restarted),
        "RustFS 关闭收据不属于服务账本中的已登记代次",
    )?;
    let path = directory.join(format!(
        "rustfs-members-{}-stopped.json",
        text(tree, "operation_id")?
    ));
    check(
        path_is(&proof["completion"]["path"], &path),
        "RustFS 完整成员关闭证明不属于本代次",
    )?;
    let value = bound(run, &proof["completion"])?;
    check(
        value["status"] == "stopped"
            && value["error_type"].is_null()
            && ["operation_id", "role", "scope_id", "supervisor", "monitor"]
                .iter()
                .all(|key| value[*key] == tree[*key])
            && value["directory"] == tree["runtime_directory"]
            && value["members"]
                .as_array()
                .is_some_and(|items| items.contains(&tree["supervisor"])),
        "RustFS 完整成员关闭证明不属于原进程树或未收尾",
    )
}

pub(super) fn reconcile(
    run: &Path,
    number: usize,
    attempts: &[Value],
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    check(
        history["closed"] == false
            && history["unsettled"]
                .as_array()
                .is_some_and(|items| items.len() == 1),
        "夹具生命周期和解必须对应唯一未结算失败",
    )?;
    let failed_number = history["unsettled"][0].as_u64().ok_or("失败阶段编号无效")?;
    let failed_index = usize::try_from(failed_number)
        .ok()
        .and_then(|number| number.checked_sub(1))
        .ok_or("失败阶段编号无效")?;
    let failed = attempts.get(failed_index).ok_or("失败阶段不存在")?;
    let is_close = result["status"] == "failed_close_reconciled";
    check(
        (is_close && history["external_recovery"].is_null() && failed["mode"] == "close")
            || (result["status"] == "failed_restart_reconciled"
                && !history["external_recovery"].is_null()
                && failed["mode"] == "restart"
                && failed_number as usize == number - 1),
        "夹具生命周期和解结果状态无效",
    )?;
    check(
        failed["stage"] == super::STAGE && failed["status"] == "failed",
        "夹具生命周期和解不能消费其他失败阶段",
    )?;
    let services = if is_close {
        json!({"redis":"stopped","rustfs":"stopped"})
    } else {
        json!({"redis":"stopped","rustfs":"external-termination-unreconciled"})
    };
    let mut fields = vec![
        "format_version",
        "kind",
        "operation",
        "status",
        "run",
        "remote_writes",
        "resources_deleted",
        "controller",
        "owner",
        "failed_attempt",
        "services",
        "reconciliation",
    ];
    if !is_close {
        fields.push("external_termination");
    }
    exact(result, &fields)?;
    check(
        result["failed_attempt"] == failed_number
            && result["owner"] == item["sources"]["state_before"]
            && result["controller"] == controller(run, item)?
            && result["services"] == services
            && path_is(
                &result["reconciliation"]["path"],
                &run.join(format!("lifecycle-{number:04}/reconciliation.json")),
            ),
        "夹具生命周期和解结果不完整",
    )?;
    let proof = bound(run, &result["reconciliation"])?;
    validate_reconciliation(
        run,
        number,
        (item, result),
        failed,
        &proof,
        history,
        is_close,
    )?;
    history["unsettled"] = json!([]);
    if is_close {
        history["closed"] = json!(true);
        history["restart_predecessor"] = item["result"].clone();
    }
    Ok(())
}

fn validate_reconciliation(
    run: &Path,
    number: usize,
    record: (&Value, &Value),
    failed: &Value,
    proof: &Value,
    history: &Value,
    close: bool,
) -> Result<()> {
    let (item, result) = record;
    let mut fields = vec![
        "format_version",
        "kind",
        "run",
        "attempt",
        "owner",
        "failed_attempt",
        "failed_error_type",
        "failed_sources",
        "failed_controller",
        "services",
        "remote_writes",
        "resources_deleted",
    ];
    fields.push(if close {
        "evidence"
    } else {
        "external_termination"
    });
    exact(proof, &fields)?;
    let kind = if close {
        "reference-fixture-failed-close-reconciliation"
    } else {
        "reference-fixture-failed-restart-reconciliation"
    };
    check(
        proof["format_version"] == 1
            && proof["kind"] == kind
            && super::evidence::path_is(&proof["run"], run)
            && proof["attempt"] == number
            && proof["owner"] == item["sources"]["state_before"]
            && proof["failed_attempt"] == failed["number"]
            && proof["failed_error_type"] == failed["error_type"]
            && proof["failed_sources"] == failed["sources"]
            && proof["failed_controller"] == controller(run, failed)?
            && proof["services"] == result["services"]
            && proof["remote_writes"] == 0
            && proof["resources_deleted"] == false,
        "夹具和解证据未精确绑定失败阶段与原进程树",
    )?;
    if close {
        let (rustfs, redis) = generation::paths(run, &history["active_generation"])?;
        let tree_path = rustfs.join("rustfs-tree.json");
        let mut services = json!({"tree":read(&tree_path)?,"runtime":read(&redis.join("runtime.json"))?,"evidence":{"rustfs_tree":binding(&tree_path)?}});
        let evidence = if proof["evidence"].get("close_recovery").is_some() {
            services["requests"] = json!({"redis":read(&redis.join("request.json"))?});
            recovery_evidence(
                run,
                failed,
                &history["active_generation"],
                &item["sources"],
                &services,
                &proof["evidence"]["close_recovery"],
            )?
        } else {
            close_evidence(
                run,
                failed,
                &history["active_generation"],
                &item["sources"],
                &services,
            )?
            .0
        };
        check(proof["evidence"] == evidence, "夹具关闭和解证据不匹配")?;
    } else {
        restart_evidence(run, failed, &item["sources"], &history["external_recovery"])?;
        let recovery = bound(run, &history["external_recovery"])?;
        check(
            proof["external_termination"] == recovery["evidence"]
                && result["external_termination"] == recovery["evidence"],
            "夹具重启和解未绑定原外部终止现场",
        )?;
        generation::external(
            &bound(run, &proof["external_termination"])?,
            run,
            &recovery["owner"],
            &history["active_generation"],
        )?;
    }
    Ok(())
}

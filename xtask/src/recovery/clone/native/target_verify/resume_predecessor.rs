//! 仅允许 marker 写入已确认、首个只读 verify 收据发布失败的一次前驱。
use super::{
    super::json as json_io, evidence_model::*, resume_model as model,
    resume_observations as observations, resume_runtime,
};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) struct Context<'a> {
    pub(super) backend: &'a Path,
    pub(super) target: &'a Path,
    pub(super) request: &'a Value,
    pub(super) descriptor: &'a Value,
    pub(super) failure: &'a Value,
    pub(super) prepared_files: &'a Value,
    pub(super) initialize_started: &'a Value,
    pub(super) creation: &'a Value,
    pub(super) reset: &'a Value,
    pub(super) operations: &'a [Value],
    pub(super) prefix: &'a [Value],
}

pub(super) struct Prior {
    pub(super) failure: Value,
    pub(super) marker: Value,
    pub(super) restart: Value,
    pub(super) binaries: Value,
    pub(super) resources: Value,
    pub(super) files: BTreeSet<String>,
    pub(super) started: Value,
    pub(super) at: Value,
}

pub(super) fn records(context: &Context<'_>, predecessor: Option<&Value>) -> Result<Option<Prior>> {
    let Context {
        backend,
        target,
        request,
        descriptor,
        failure,
        prepared_files,
        initialize_started,
        creation,
        reset,
        operations,
        prefix,
    } = context;
    let all = model::resume_names(target)?;
    if predecessor.is_none() && all.is_empty() {
        return Ok(None);
    }
    let next = operations
        .get(prefix.len())
        .ok_or("迁移初始化只允许一个未确认只读操作的首次续作前驱")?;
    check(
        next["write"] == false,
        "迁移初始化只允许一个未确认只读操作的首次续作前驱",
    )?;
    let failure_paths = names(target, r"^resume-initialize-.*\.failure\.json$")?;
    let failure_path = if let Some(value) = predecessor {
        bound(backend, value)?
    } else {
        check(
            failure_paths.len() == 1
                && names(target, r"^resume-initialize-.*\.intent\.json$")?.len() == 1
                && names(target, r"^resume-initialize-.*\.started\.json$")?.len() == 1
                && names(target, r"^resume-initialize-.*\.confirmed\.json$")?.is_empty(),
            "迁移初始化只允许一个未确认只读操作的首次续作前驱",
        )?;
        target.join(failure_paths.first().ok_or("迁移前驱失败缺失")?)
    };
    check(
        failure_path.parent() == Some(*target),
        "迁移后继的前驱失败路径无效",
    )?;
    let name = failure_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("迁移前驱文件名无效")?;
    let pattern = Regex::new(r"^resume-initialize-([a-f0-9]{32})\.failure\.json$")?;
    let captures = pattern
        .captures(name)
        .ok_or("迁移后继的前驱 attempt 命名无效")?;
    let attempt = &captures[1];
    let Evidence {
        intent_path,
        started_path,
        intent,
        started,
        failed,
        op,
        marker,
        files,
        path_failure,
    } = load(context, &failure_path, attempt, next)?;
    check(
        path_failure
            && intent["format_version"] == 1
            && intent["kind"] == "devex-clone-migration-resume-intent"
            && intent["attempt"] == attempt
            && intent["request"] == **descriptor
            && intent["failure"] == **failure
            && intent["prepare"] == binding(backend, &target.join("prepare.json"))?
            && intent["initialize_started"] == **initialize_started
            && intent["reset"] == **reset
            && intent["creation"] == **creation
            && intent["migration_prefix"] == json!(prefix)
            && intent["prepared_files"] == **prepared_files
            && intent["next_operation"] == next["id"]
            && intent["protected_binaries"] == model::binaries(backend, request)?
            && intent["remote_write_operations_before_resume"].as_u64() == Some(0)
            && started["format_version"] == 1
            && started["kind"] == "devex-clone-migration-resume-started"
            && started["attempt"] == attempt
            && started["intent"] == binding(backend, &intent_path)?
            && valid_failed(
                &failed,
                attempt,
                &binding(backend, &intent_path)?,
                &binding(backend, &started_path)?,
                context,
            )?
            && op["format_version"] == 1
            && op["kind"] == "devex-clone-migration-operation-intent"
            && op["attempt"] == attempt
            && op["index"].as_u64() == Some(prefix.len() as u64)
            && op["resume"] == binding(backend, &intent_path)?
            && op["failure"] == **failure
            && op["operation"] == model::operation(next)
            && !marker.confirmation.is_null()
            && (predecessor.is_some() || all == files)
            && ordered(&[
                &read(backend, &target.join("failure.json"))?["at"],
                &intent["at"],
                &started["at"],
            ])
            && ordered(&[&marker.at, &op["at"], &failed["at"]])
            && (predecessor.is_some()
                || observations::stage(
                    &names(target, r"^.*$")?,
                    json_io::text(&next["stage"], "阶段")?,
                )?
                .is_empty()),
        "迁移后继的前驱失败、只读操作或证据闭集无效",
    )?;
    let marker_value = read(backend, &bound(backend, &marker.confirmation)?)?;
    let resources = json!({"databases":started["resources_before"]["databases"],"objects":started["resources_before"]["objects"],"redis":marker_value["after"]});
    model::owned(backend, request, &resources, true)?;
    Ok(Some(Prior {
        failure: binding(backend, &failure_path)?,
        marker: marker.confirmation,
        restart: resume_runtime::fixture(&started["storage_runtime"], &started["cache_runtime"])?,
        binaries: intent["protected_binaries"].clone(),
        resources,
        files,
        at: failed["at"].clone(),
        started,
    }))
}

struct Evidence {
    intent_path: PathBuf,
    started_path: PathBuf,
    intent: Value,
    started: Value,
    failed: Value,
    op: Value,
    marker: resume_runtime::Markers,
    files: BTreeSet<String>,
    path_failure: bool,
}

fn load(
    context: &Context<'_>,
    failure_path: &Path,
    attempt: &str,
    next: &Value,
) -> Result<Evidence> {
    let Context {
        backend,
        target,
        request,
        prefix,
        ..
    } = context;
    let name = failure_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("迁移前驱文件名无效")?;
    let intent_path = target.join(format!("resume-initialize-{attempt}.intent.json"));
    let started_path = target.join(format!("resume-initialize-{attempt}.started.json"));
    let intent = read(backend, &intent_path)?;
    let started = read(backend, &started_path)?;
    let failed = read(backend, failure_path)?;
    json_io::exact(&intent, model::INTENT, "迁移前驱 intent")?;
    json_io::exact(&started, model::STARTED, "迁移前驱 started")?;
    let path_failure = path_failure(target, next, &failed)?;
    let op_path = target.join(format!(
        "resume-migrate-{attempt}-{:02}.intent.json",
        prefix.len()
    ));
    let op = read(backend, &op_path)?;
    json_io::exact(&op, model::OP_INTENT, "迁移前驱操作")?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    resume_runtime::started(backend, &prepared["generation"], &started)?;
    let marker = resume_runtime::markers(
        backend,
        target,
        request,
        attempt,
        &intent_path,
        &started_path,
        &started,
    )?;
    model::owned(backend, request, &started["resources_before"], false)?;
    let mut files = BTreeSet::from([
        name.to_owned(),
        model::descriptor_name(&binding(backend, &intent_path)?)?,
        model::descriptor_name(&binding(backend, &started_path)?)?,
        model::descriptor_name(&binding(backend, &op_path)?)?,
    ]);
    files.extend(marker.files.clone());
    Ok(Evidence {
        intent_path,
        started_path,
        intent,
        started,
        failed,
        op,
        marker,
        files,
        path_failure,
    })
}

fn path_failure(target: &Path, next: &Value, failed: &Value) -> Result<bool> {
    let diagnostic = failed.get("phase").is_some() || failed.get("os_error").is_some();
    let mut fields = model::FAILURE.to_vec();
    if diagnostic {
        fields.extend(["phase", "os_error"]);
    }
    json_io::exact(failed, &fields, "迁移前驱失败")?;
    let mut active_fields = vec!["id", "index", "intent", "command_receipts"];
    if diagnostic {
        active_fields.extend(["phase", "receipt_path_length", "receipt_path_limit_risk"]);
        json_io::exact(
            &failed["os_error"],
            &["errno", "winerror", "filename_role", "filename_length"],
            "操作系统错误",
        )?;
    }
    json_io::exact(
        &failed["active_operation"],
        &active_fields,
        "迁移前驱活动操作",
    )?;
    let length = target
        .join(format!(
            "{}-{}.command.json",
            json_io::text(&next["stage"], "操作阶段")?,
            "0".repeat(32)
        ))
        .to_string_lossy()
        .chars()
        .count();
    let path_failure = length >= 260
        && (!diagnostic
            || (failed["phase"] == "command-receipt-publish"
                && failed["active_operation"]["phase"] == "command-receipt-publish"
                && failed["active_operation"]["receipt_path_length"].as_u64()
                    == Some(length as u64)
                && failed["active_operation"]["receipt_path_limit_risk"] == true
                && failed["os_error"]["filename_role"] == "command_receipt"
                && failed["os_error"]["errno"].as_i64() == Some(2)
                && (failed["os_error"]["winerror"].is_null()
                    || [Some(2), Some(3), Some(206)]
                        .contains(&failed["os_error"]["winerror"].as_i64()))
                && failed["os_error"]["filename_length"].as_u64() == Some(length as u64)));
    Ok(path_failure)
}

fn valid_failed(
    failed: &Value,
    attempt: &str,
    intent: &Value,
    started: &Value,
    context: &Context<'_>,
) -> Result<bool> {
    let active = &failed["active_operation"];
    Ok(failed["format_version"] == 1
        && failed["kind"] == "devex-clone-migration-resume-failure"
        && failed["attempt"] == attempt
        && failed["intent"] == *intent
        && failed["source_failure"] == *context.failure
        && failed["prepared_files"] == *context.prepared_files
        && failed["started"] == *started
        && failed["error_type"] == "FileNotFoundError"
        && failed["confirmed_operations"] == json!([])
        && failed["remote_write_operations_confirmed"].as_u64() == Some(1)
        && failed["unknown_write_operation"].is_null()
        && active["id"] == context.operations[context.prefix.len()]["id"]
        && active["index"].as_u64() == Some(context.prefix.len() as u64)
        && active["intent"]
            == binding(
                context.backend,
                &context.target.join(format!(
                    "resume-migrate-{attempt}-{:02}.intent.json",
                    context.prefix.len()
                )),
            )?
        && active["command_receipts"] == json!([]))
}

pub(super) fn observation_names(
    context: &Context<'_>,
    current: &Value,
    baseline: &Value,
    started: &Value,
) -> Result<BTreeSet<String>> {
    let old = observations::file_map(baseline)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    let additions = observations::file_map(current)?
        .into_keys()
        .filter(|name| !old.contains(name))
        .collect::<BTreeSet<_>>();
    let pattern = Regex::new(r"^storage-layout-[a-f0-9]{32}\.json$")?;
    let original = old
        .iter()
        .filter(|name| pattern.is_match(name))
        .map(|name| read(context.backend, &context.target.join(name)))
        .collect::<Result<Vec<_>>>()?;
    check(
        original.len() == 2 && original[0] == original[1],
        "迁移后继的存储 prepare 基线无效",
    )?;
    let mut layout = original[0].clone();
    let runtime = &started["storage_runtime"];
    if !runtime.is_null() {
        layout["process_receipt"] = runtime["storage"]["process_receipt"].clone();
        layout["launch_receipt"] = runtime["storage"]["launch_receipt"].clone();
        layout["runtime_transition"] = runtime.clone();
    }
    let mut selected = BTreeSet::new();
    for name in additions.iter().filter(|name| pattern.is_match(name)) {
        if read(context.backend, &context.target.join(name))? == layout {
            selected.insert(name.clone());
        }
    }
    check(
        selected.len() == 2,
        "迁移后继缺少首次续作的两轮存储代次观察",
    )?;
    redis_observations(context, &old, &additions, started, &mut selected)?;
    for db in json_io::array(&context.request["target"]["databases"], "数据库")? {
        let stage = format!("mysql-{}", json_io::text(&db["key"], "数据库键")?);
        let before = observations::stage(&old, &stage)?;
        check(before.len() == 1, "迁移后继的 MySQL prepare 基线无效")?;
        let before = observations::receipt(context.backend, context.target, &before[0])?;
        let stdout = format!(
            "{}\n{}\n",
            json_io::text(&db["server_uuid"], "UUID")?,
            json_io::text(&db["database"], "数据库")?
        );
        let mut found = None;
        for name in observations::stage(&additions, &stage)? {
            let value = observations::receipt(context.backend, context.target, &name)?;
            if value["command"] == before["command"] && value["stdout"] == stdout {
                found = Some(name);
                break;
            }
        }
        selected.insert(found.ok_or("迁移后继缺少首次续作的数据库前像观察")?);
    }
    Ok(selected)
}

fn redis_observations(
    context: &Context<'_>,
    old: &BTreeSet<String>,
    additions: &BTreeSet<String>,
    started: &Value,
    selected: &mut BTreeSet<String>,
) -> Result<()> {
    let baseline = observations::stage(old, "redis-kernel")?
        .iter()
        .map(|name| observations::receipt(context.backend, context.target, name))
        .collect::<Result<Vec<_>>>()?;
    let mut expected = BTreeMap::new();
    let pid = started["cache_runtime"]["redis"]["pid"]
        .as_u64()
        .ok_or("缓存重启 PID 无效")?;
    let pattern = Regex::new(r"^/proc/[0-9]+/(stat|exe)$")?;
    for value in &baseline {
        let mut command = value["command"].clone();
        let parts = command.as_array_mut().ok_or("Redis 命令无效")?;
        let last = parts.last_mut().ok_or("Redis 命令为空")?;
        let kind = pattern
            .captures(json_io::text(last, "Redis 命令末项")?)
            .map(|capture| capture[1].to_owned());
        if let Some(kind) = &kind {
            *last = json!(format!("/proc/{pid}/{kind}"));
        }
        expected.insert(serde_json::to_string(&command)?, (value, kind));
    }
    check(
        baseline.len() == 10 && expected.len() == 5,
        "迁移后继的 Redis prepare 基线无效",
    )?;
    let mut candidates = BTreeMap::<String, Vec<(String, Value)>>::new();
    for name in observations::stage(additions, "redis-kernel")? {
        let value = observations::receipt(context.backend, context.target, &name)?;
        candidates
            .entry(serde_json::to_string(&value["command"])?)
            .or_default()
            .push((name, value));
    }
    for (command, (before, kind)) in expected {
        let matches = candidates
            .get(&command)
            .ok_or("迁移后继缺少首次续作的两轮 Redis 代次观察")?;
        check(
            matches.len() >= 2,
            "迁移后继缺少首次续作的两轮 Redis 代次观察",
        )?;
        for (name, value) in matches.iter().take(2) {
            let valid = if kind.as_deref() == Some("stat") {
                let (head, _) = observations::process_identity(json_io::text(
                    &before["stdout"],
                    "Redis 输出",
                )?)?;
                let (_, comm) = head.split_once(' ').ok_or("Redis 进程名称缺失")?;
                observations::process_identity(json_io::text(&value["stdout"], "Redis 输出")?)?
                    == (
                        format!("{pid} {comm}"),
                        json_io::text(
                            &started["cache_runtime"]["redis"]["started"],
                            "Redis 启动时间",
                        )?
                        .to_owned(),
                    )
            } else {
                value["stdout"] == before["stdout"]
            };
            check(valid, "迁移后继的 Redis 重启观察与固定代次不符")?;
            selected.insert(name.clone());
        }
    }
    Ok(())
}

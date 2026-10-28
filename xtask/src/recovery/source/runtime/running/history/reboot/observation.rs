//! 正式观察和历史复核共享同一原授权、配置及三成员事实；不触发控制动作。
use super::{CLOSE, completed, host, ready, stops};
use crate::{
    Result,
    recovery::{
        clone::native::{
            json as document,
            lifecycle::{lock, state, status},
            resources::common,
            seed::generation::coordinator,
            target_verify::authority,
        },
        fixture_control::artifact::runtime,
        source::evidence::{binding, running::roles},
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::source::evidence) fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["backend", "directory"], "重启观察参数")?;
    let backend = binding::path(&args["backend"])?;
    let run = binding::path(&args["directory"])?;
    common::local_directory(&backend, &run, "重启观察运行")?;
    let before = common::binding(&run.join("state.json"))?;
    let manifest = common::binding(&run.join("manifest.json"))?;
    let ledger = state::load(&run, true)?;
    let attempts = state::attempts(&ledger)?;
    let authority = completed::read(&backend, &run, attempts)?
        .ok_or("缺少已收尾并绑定 C69 授权的唯一源代次 START")?;
    let observed = read(&backend, &run, attempts, &authority["completed_successor"])?;
    common::bound_file(&backend, &before, "重启观察原账本")?;
    common::bound_file(&backend, &manifest, "重启观察原清单")?;
    Ok(observed)
}

pub(super) fn read(backend: &Path, run: &Path, attempts: &[Value], start: &Value) -> Result<Value> {
    let state_binding = common::binding(&run.join("state.json"))?;
    if state::attempts(&state::load(run, true)?)? != attempts {
        return Err("重启观察必须采用当前完整账本".into());
    }
    let controller = current_restart(backend, run, attempts, start)?;
    let facts = generation(backend, run, start)?;
    let output = binding::path(&facts["output"])?;
    let directory = output.join("runtime");
    let execution = binding::path(&facts["request"]["execution_backend"])?;
    let frozen = common::binding(&directory.join("runtime.json"))?;
    let mut bindings = vec![state_binding, frozen];
    for role in ["api", "worker"] {
        for name in [format!("{role}.json"), format!("{role}-tree.json")] {
            bindings.push(common::binding(&directory.join(name))?);
        }
    }
    let contract = runtime::verify_configuration(&execution, &directory)?;
    roles::directory(&directory, &facts["intent"]["operations"])?;
    let evidence = running_evidence(backend, &output, &facts)?;
    bindings.push(facts["receipt"]["runtime_evidence"].clone());
    roles::observe(
        &directory,
        &evidence,
        &facts["intent"],
        &json!({"scope_id":contract.scope}),
        &contract.receipt,
        false,
    )?;
    let prefix = failed_prefix(backend, &output.join("verification"))?;
    let proof = host::proof()?;
    let boot = proof["boot_ticks"].as_u64().ok_or("主机证明缺少启动刻度")?;
    let urls = endpoints(&contract)?;
    let (role_facts, mut blockers) =
        role_facts(&directory, &facts["intent"], &contract, &urls, boot)?;
    let (storage, storage_blockers) = storage(&facts["request"], boot)?;
    blockers.extend(storage_blockers);
    if runtime::verify_configuration(&execution, &directory)?.receipt != contract.receipt {
        return Err("重启观察期间绑定配置变化；保留现场，禁止重放".into());
    }
    if generation(backend, run, start)? != facts
        || failed_prefix(backend, &output.join("verification"))? != prefix
    {
        return Err("重启观察期间原请求、意图或失败前缀变化；保留现场，禁止重放".into());
    }
    for binding in &bindings {
        common::bound_file(backend, binding, "重启观察前后绑定")?;
    }
    roles::directory(&directory, &facts["intent"]["operations"])?;
    common::bound_file(backend, &start["result"], "重启源代次收据")?;
    if current_restart(backend, run, attempts, start)? != controller {
        return Err("重启观察期间原收尾、控制器或运行锁变化；保留现场，禁止重放".into());
    }
    host::require_fresh(&proof)?;
    let later = attempts
        .iter()
        .filter(|row| {
            row["number"].as_u64() > start["number"].as_u64()
                && !(row["stage"] == "seed-runtime" && row["mode"] == CLOSE)
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"start":start,"later":later,"facts":facts,"prefix":prefix,
        "proof":proof,"roles":role_facts,"storage":storage,"blockers":blockers}),
    )
}

fn history(attempts: &[Value], start: &Value) -> Result<()> {
    let number = start["number"].as_u64().ok_or("C70 源代次序号无效")?;
    let running = attempts
        .iter()
        .filter(|row| row["status"] == "running")
        .collect::<Vec<_>>();
    if start["status"] != "passed"
        || running.len() > 1
        || running.iter().any(|row| {
            !(row["stage"] == "seed-runtime" && row["mode"] == CLOSE
                || row["mode"] == "restart"
                    && matches!(
                        row["stage"].as_str(),
                        Some("storage-target" | "cache-target")
                    ))
        })
    {
        return Err("重启观察缺少唯一已收尾成功 START 或仍有未授权控制动作".into());
    }
    if attempts.iter().any(|row| {
        row["number"].as_u64().is_some_and(|n| n > number)
            && row["stage"] == "seed-runtime"
            && row["mode"] != CLOSE
    }) {
        return Err("源代次之后存在停止、恢复或其他源阶段，不能按重启中断观察".into());
    }
    Ok(())
}

/// 只容纳原账本中已发布收尾之后的当前受管阶段；不授予执行资格。
fn current_restart(backend: &Path, run: &Path, attempts: &[Value], start: &Value) -> Result<Value> {
    history(attempts, start)?;
    let Some(current) = attempts
        .iter()
        .find(|row| row["status"] == "running" && row["stage"] != "seed-runtime")
    else {
        return Ok(Value::Null);
    };
    let state_binding = common::binding(&run.join("state.json"))?;
    if state::attempts(&state::load(run, true)?)? != attempts {
        return Err("受管重启必须采用原账本当前完整前像".into());
    }
    if attempts.last() != Some(current)
        || !current["result"].is_null()
        || !current["error_type"].is_null()
        || !current["finished_at"].is_null()
    {
        return Err("受管重启必须为原账本最后唯一未收尾阶段".into());
    }
    let closures = attempts
        .iter()
        .filter(|row| row["stage"] == "seed-runtime" && row["mode"] == CLOSE)
        .collect::<Vec<_>>();
    let published = closures
        .iter()
        .copied()
        .filter(|row| row["status"] == "passed")
        .collect::<Vec<_>>();
    let [closed] = published.as_slice() else {
        return Err("受管重启缺少唯一成功原收尾".into());
    };
    if closures
        .iter()
        .any(|row| *row != *closed && (row["status"] != "failed" || !row["result"].is_null()))
    {
        return Err("受管重启存在未知或未收尾的重复收尾记录".into());
    }
    let number = start["number"].as_u64().ok_or("原 START 序号无效")?;
    if closed["number"].as_u64() != number.checked_add(1) {
        return Err("受管重启收尾不是原 START 后相邻阶段".into());
    }
    let tail = attempts
        .iter()
        .filter(|row| row["number"].as_u64() > closed["number"].as_u64())
        .collect::<Vec<_>>();
    let shapes = [("storage-target", "restart"), ("cache-target", "restart")];
    if tail.is_empty() || tail.len() > shapes.len() {
        return Err("受管重启后续代次集合无效".into());
    }
    for (offset, (row, (stage, mode))) in tail.iter().zip(shapes).enumerate() {
        if row["number"].as_u64() != number.checked_add(u64::try_from(offset)? + 2)
            || row["stage"] != stage
            || row["mode"] != mode
            || row["status"]
                != if offset + 1 == tail.len() {
                    "running"
                } else {
                    "passed"
                }
        {
            return Err("受管重启必须继承原收尾后的相邻存储及缓存阶段".into());
        }
    }
    let issued = super::closure(backend, run, closed, start)?;
    let owner = restart_owner(backend, run, current)?;
    common::bound_file(backend, &state_binding, "受管重启账本后像")?;
    Ok(json!({"controller":owner,"closure":issued["binding"],"closure_result":issued["result"]}))
}

fn restart_owner(backend: &Path, run: &Path, current: &Value) -> Result<Value> {
    let number = current["number"].as_u64().ok_or("当前受管重启序号无效")?;
    let (controller, owner) = state::controller_record(run, number, current)?;
    let locked = lock::owner_observation(run)?.ok_or("当前受管重启缺少原 run.lock")?;
    if owner != locked.1 {
        return Err("当前受管重启 controller 与 run.lock owner 不同".into());
    }
    let observed = status::observe_controller(run, &state::load(run, true)?)?;
    if observed["owner"] != locked.0
        || observed["process_matches"] != true
        || observed["process_missing"] != false
    {
        return Err("当前受管重启 owner 的实际创建身份缺失、复用或未知".into());
    }
    common::bound_file(backend, &controller, "受管重启控制器后像")?;
    if lock::owner_observation(run)? != Some(locked.clone()) {
        return Err("受管重启核验期间运行锁变化".into());
    }
    Ok(json!({"controller":controller,"owner":locked.0}))
}

fn generation(backend: &Path, run: &Path, start: &Value) -> Result<Value> {
    let number = start["number"].as_u64().ok_or("C70 源代次序号无效")?;
    let receipt = stops::result(backend, run, start)?;
    document::exact(
        &receipt,
        coordinator::receipts::START_FIELDS,
        "重启源代次收据",
    )?;
    if receipt["status"] != "seed_source_generation_running"
        || receipt["restore_qualified"] != false
    {
        return Err("源代次收据状态不是运行中的冻结来源".into());
    }
    let request = common::read_bound(backend, &receipt["request"], "重启源请求")?;
    coordinator::request(&request)?;
    // 仍核对原私有文件绑定；其中的环境值不能替代 runtime 指定的配置目录或端点。
    common::bound_file(backend, &request["source_environment"], "重启原私有文件")?;
    for field in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        if receipt[field] != request[field] {
            return Err("重启 START 与原请求的来源或存储授权不一致".into());
        }
    }
    let output = run.join(document::text(&receipt["generation_id"], "源代次目录")?);
    common::local_directory(backend, &output, "重启源代次")?;
    if output != run.join(format!("g{number:04}"))
        || document::read(&output.join("request.json"))? != request
    {
        return Err("源代次目录、请求副本与账本不一致".into());
    }
    let intent = common::read_bound(backend, &receipt["intent"], "重启源启动意图")?;
    document::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "request",
            "runtime_directory",
            "operations",
        ],
        "重启源启动意图",
    )?;
    document::exact(&intent["operations"], &["api", "worker"], "重启源角色集合")?;
    if intent["format_version"] != 1
        || intent["kind"] != "seed-source-runtime-intent"
        || intent["request"] != common::binding(&output.join("request.json"))?
        || receipt["intent"] != common::binding(&output.join("intent.json"))?
        || common::read_bound(backend, &intent["request"], "源启动意图请求")? != request
        || intent["runtime_directory"] != json!(output.join("runtime"))
    {
        return Err("源代次启动意图与收据或角色集合不一致".into());
    }
    Ok(json!({"receipt":receipt,"request":request,"output":output,"intent":intent}))
}

fn running_evidence(backend: &Path, output: &Path, facts: &Value) -> Result<Value> {
    let receipt = &facts["receipt"];
    let evidence = common::read_bound(backend, &receipt["runtime_evidence"], "重启原运行证据")?;
    document::exact(
        &evidence,
        &[
            "format_version",
            "kind",
            "intent",
            "runtime",
            "roles",
            "physical_binding",
        ],
        "重启原运行证据",
    )?;
    if receipt["runtime_evidence"] != common::binding(&output.join("running-evidence.json"))?
        || evidence["format_version"] != 1
        || evidence["kind"] != "seed-source-runtime-running"
        || evidence["intent"] != receipt["intent"]
        || evidence["runtime"] != common::binding(&output.join("runtime/runtime.json"))?
    {
        return Err("重启观察原运行合同或角色证据不属于原 START 绑定".into());
    }
    Ok(evidence)
}

fn failed_prefix(backend: &Path, directory: &Path) -> Result<Value> {
    let allowed = ready::verify(&json!({"action":"failed-entries","arguments":{}}))?;
    let allowed = document::array(&allowed, "重启中断失败目录集合")?
        .iter()
        .map(|v| document::text(v, "失败目录名称"))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    common::local_directory(backend, directory, "重启中断失败目录")?;
    let names = crate::recovery::runtime::dataset_authority::reads::entries(directory)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let names = names
        .iter()
        .map(|name| name.to_str().ok_or("失败目录名称不是 UTF-8".into()))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    if names != allowed
        || document::read(&directory.join("failed.json"))?
            != json!({"status":"failed","error":"ValueError"})
    {
        return Err("来源验收失败前缀不是已知未清理缓存失败的精确集合".into());
    }
    let producer = ready::verify(
        &json!({"action":"producer-bindings","arguments":{"backend":backend,"directory":directory}}),
    )?;
    Ok(
        json!({"directory":directory,"failed":common::binding(&directory.join("failed.json"))?,
        "producer":producer,"audit":common::binding(&directory.join("audit/login-audit.json"))?,
        "before":common::binding(&directory.join("before/image.json"))?,"manifest":names}),
    )
}

fn endpoints(contract: &runtime::Binding) -> Result<Value> {
    let app = contract
        .table
        .get("app")
        .and_then(toml::Value::as_table)
        .ok_or("重启源配置缺少 app")?;
    let host = app
        .get("host")
        .and_then(toml::Value::as_str)
        .ok_or("重启源配置缺少 app.host")?;
    let port = app
        .get("port")
        .and_then(toml::Value::as_integer)
        .filter(|v| (1..=65535).contains(v))
        .ok_or("重启源配置 app.port 无效")?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Ok(json!({"api":format!("http://{host}:{port}"),"worker":contract.receipt["worker_ready_url"]}))
}

fn role_facts(
    directory: &Path,
    intent: &Value,
    contract: &runtime::Binding,
    urls: &Value,
    boot: u64,
) -> Result<(Value, Vec<String>)> {
    let mut result = serde_json::Map::new();
    let mut blockers = Vec::new();
    for role in ["api", "worker"] {
        let tree = roles::tree(directory, role, &contract.receipt["scope_id"])?;
        if tree["operation_id"] != intent["operations"][role]
            || roles::process(directory, role, &contract.receipt["scope_id"])? != tree["process"]
        {
            return Err("进程树不属于 C70 启动意图或原创建身份".into());
        }
        let members = members(&tree, boot)?;
        let all_exited = members.values().all(|v| v["exited"] == true);
        let ports = ports(&[document::text(&urls[role], "重启角色端点")?]);
        if !all_exited {
            blockers.push(format!("{role} 代次无法证明已随主机重启退出"));
        }
        if ports.iter().any(|v| v["closed"] != true) {
            blockers.push(format!("{role} 端点没有明确关闭"));
        }
        let path = directory.join(format!(
            "{role}-members-{}-stopped.json",
            document::text(&tree["operation_id"], "原角色操作")?
        ));
        let completion = if path.try_exists()? {
            common::binding(&path)?
        } else {
            Value::Null
        };
        result.insert(
            role.into(),
            json!({"operation_id":tree["operation_id"],"members":members,
            "all_exited":all_exited,"ports":ports,"completion":completion}),
        );
    }
    Ok((Value::Object(result), blockers))
}

fn members(tree: &Value, boot: u64) -> Result<serde_json::Map<String, Value>> {
    let mut members = serde_json::Map::new();
    let mut pids = std::collections::BTreeSet::new();
    for member in ["process", "supervisor", "monitor"] {
        let exited = host::exited(&tree[member], boot)?;
        let pid = tree[member]["pid"].as_u64().ok_or("原成员 PID 无效")?;
        if !pids.insert(pid) {
            return Err("原三成员创建身份不能复用同一 PID".into());
        }
        members.insert(
            member.into(),
            json!({"identity":tree[member],"exited":exited}),
        );
    }
    Ok(members)
}

fn ports(urls: &[&str]) -> Vec<Value> {
    urls.iter()
        .map(|url| match authority::require_closed(url) {
            Ok(()) => json!({"url":url,"closed":true}),
            Err(_) => json!({"url":url,"closed":false,"error_type":"ValueError"}),
        })
        .collect()
}

fn storage(request: &Value, boot: u64) -> Result<(Value, Vec<String>)> {
    let storage = &request["current_storage"];
    let identity = &storage["storage"]["identity"];
    let exited = host::exited(identity, boot)?;
    let ports = ports(&[
        document::text(&storage["api_url"], "旧存储 API")?,
        document::text(&storage["console_url"], "旧存储 Console")?,
    ]);
    let mut blockers = Vec::new();
    if !exited {
        blockers.push("对象存储代次无法证明已随主机重启退出".into());
    }
    if ports.iter().any(|v| v["closed"] != true) {
        blockers.push("对象存储端点没有明确关闭".into());
    }
    Ok((
        json!({"attempt":storage["attempt"],"identity":identity,"exited":exited,
        "request":storage["request"],"ports":ports}),
        blockers,
    ))
}

pub(super) fn require_closed(observed: &Value) -> Result<()> {
    if !document::array(&observed["blockers"], "重启阻塞项")?.is_empty() {
        return Err("主机重启角色或存储尚无完整退出与端口关闭证明；保留现场，禁止重放".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/source/runtime/reboot.rs"]
mod tests;

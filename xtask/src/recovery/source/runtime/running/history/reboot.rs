//! 重启段只消费现有收尾和登记证据；不签发收尾、不启动或停止任何资源。
use super::{completed, stops};
use crate::recovery::source::evidence::ready;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, json as document,
        resources::{common, storage::registered},
        seed::generation::coordinator,
    },
};
use serde_json::{Value, json};
use std::path::Path;
#[path = "reboot/host.rs"]
mod host;

const CLOSE: &str = "source-generation-reboot-close";

pub(super) fn tail(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    tail: &[Value],
    archive: &Value,
    descriptor: &Value,
) -> Result<Option<Value>> {
    let closures = attempts
        .iter()
        .filter(|row| row["stage"] == "seed-runtime" && row["mode"] == CLOSE)
        .collect::<Vec<_>>();
    if closures.is_empty() {
        return Ok(None);
    }
    let published = closures
        .iter()
        .copied()
        .filter(|row| row["status"] == "passed" && !row["result"].is_null())
        .collect::<Vec<_>>();
    if published.len() != 1
        || closures
            .iter()
            .any(|row| !published.contains(row) && row["status"] != "failed")
    {
        return Err("主机重启收尾不能重复、失败或未收尾".into());
    }
    let authority = completed::read(backend, run, attempts)?
        .ok_or("存在重启收尾收据但缺少已收尾的 C70 源代次")?;
    let start = &authority["completed_successor"];
    let start_number = start["number"].as_u64().ok_or("C70 源代次序号无效")?;
    let rows = tail
        .iter()
        .filter(|row| row["number"].as_u64().is_some_and(|n| n > start_number))
        .cloned()
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(None);
    }
    let shapes = [
        ("seed-runtime", CLOSE),
        ("storage-target", "restart"),
        ("cache-target", "restart"),
    ];
    if rows.len() != shapes.len() {
        return Err("主机重启收尾后的目标存储与缓存重启不完整".into());
    }
    for (offset, (row, (stage, mode))) in rows.iter().zip(shapes).enumerate() {
        if row["number"].as_u64() != start_number.checked_add(u64::try_from(offset + 1)?)
            || row["stage"] != stage
            || row["mode"] != mode
            || row["status"] != "passed"
        {
            return Err("主机重启收尾后的目标存储与缓存重启不完整或不相邻".into());
        }
    }
    observe(backend, run, attempts, start)?;
    let issued = closure(backend, run, published[0])?;
    if published[0]["number"] != rows[0]["number"] {
        return Err("重启段没有采用当前唯一收尾收据".into());
    }
    Ok(Some(restarted(
        backend, run, &rows, archive, descriptor, &issued,
    )?))
}

fn restarted(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    archive: &Value,
    descriptor: &Value,
    issued: &Value,
) -> Result<Value> {
    let before = rows[2]["number"]
        .as_u64()
        .and_then(|n| n.checked_add(1))
        .ok_or("重启段缓存序号溢出")?;
    let storage = registered::registered(backend, run, CloneSide::Target, Some(before))?
        .unwrap_or(Value::Null);
    if storage["attempt"] != rows[1]["number"] || storage["restart_result"] != rows[1]["result"] {
        return Err("收尾后的对象存储重启没有绑定同一登记代次".into());
    }
    let cache = cache::binding::query(
        backend,
        run,
        None,
        &json!(before),
        &json!([descriptor, archive["receipt"]["review_successor"]]),
    )?;
    if cache["attempt"] != rows[2]["number"] || cache["restart_result"] != rows[2]["result"] {
        return Err("收尾后的缓存重启没有绑定同一登记代次".into());
    }
    common::bound_file(backend, &issued["binding"], "重启收尾")?;
    Ok(
        json!({"records":rows,"storage":storage,"cache":cache,"closure":issued["binding"],
        "baseline":issued["receipt"]["baseline"],"deltas":issued["receipt"]["declared_deltas"]}),
    )
}

fn observe(backend: &Path, run: &Path, attempts: &[Value], start: &Value) -> Result<()> {
    if attempts.iter().any(|row| {
        row["status"] == "running" && (row["stage"] != "seed-runtime" || row["mode"] != CLOSE)
    }) || attempts
        .iter()
        .filter(|row| row["status"] == "running")
        .count()
        > 1
        || start["status"] != "passed"
    {
        return Err("重启观察缺少唯一已收尾成功 START 或仍有未收尾控制动作".into());
    }
    let number = start["number"].as_u64().ok_or("C70 源代次序号无效")?;
    if attempts.iter().any(|row| {
        row["number"].as_u64().is_some_and(|n| n > number)
            && row["stage"] == "seed-runtime"
            && row["mode"] != CLOSE
    }) {
        return Err("源代次之后存在停止、恢复或其他源阶段，不能按重启中断观察".into());
    }
    let value = stops::result(backend, run, start)?;
    document::exact(
        &value,
        coordinator::receipts::START_FIELDS,
        "重启源代次收据",
    )?;
    if value["status"] != "seed_source_generation_running" || value["restore_qualified"] != false {
        return Err("源代次收据状态不是运行中的冻结来源".into());
    }
    let request = common::read_bound(backend, &value["request"], "重启源请求")?;
    coordinator::request(&request)?;
    let output = run.join(document::text(&value["generation_id"], "源代次目录")?);
    if output != run.join(format!("g{number:04}"))
        || document::read(&output.join("request.json"))? != request
    {
        return Err("源代次目录、请求副本与账本不一致".into());
    }
    let intent = common::read_bound(backend, &value["intent"], "重启源启动意图")?;
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
    let runtime = output.join("runtime");
    if common::read_bound(backend, &intent["request"], "源启动意图请求")? != request
        || intent["runtime_directory"] != json!(runtime)
    {
        return Err("源代次启动意图与收据或角色集合不一致".into());
    }
    failed_prefix(backend, &output.join("verification"))?;
    let boot = host::boot()?;
    let contract = document::read(&runtime.join("runtime.json"))?;
    for role in ["api", "worker"] {
        let tree = super::super::roles::tree(&runtime, role, &contract["scope_id"])?;
        if tree["operation_id"] != intent["operations"][role] {
            return Err("进程树不属于 C70 启动意图".into());
        }
        for member in ["process", "supervisor", "monitor"] {
            host::exited(&tree[member], boot)?;
        }
    }
    closed_ports(&request, &contract, boot)?;
    common::bound_file(backend, &start["result"], "重启源代次收据")?;
    Ok(())
}

fn failed_prefix(backend: &Path, directory: &Path) -> Result<()> {
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
    ready::verify(
        &json!({"action":"producer-bindings","arguments":{"backend":backend,"directory":directory}}),
    )?;
    for name in ["audit/login-audit.json", "before/image.json"] {
        common::binding(&directory.join(name))?;
    }
    Ok(())
}

fn closure(backend: &Path, run: &Path, record: &Value) -> Result<Value> {
    let result = common::read_bound(backend, &record["result"], "重启收尾结果")?;
    document::exact(
        &result,
        &[
            "status",
            "start",
            "request",
            "closure",
            "baseline",
            "reboot",
            "failed_prefix",
            "declared_deltas",
            "producer",
            "verification_qualified",
            "replay_allowed",
            "remote_writes",
            "restore_qualified",
        ],
        "重启收尾结果",
    )?;
    if result["status"] != "seed_source_generation_reboot_closed" {
        return Err("主机重启收尾阶段结果状态无效".into());
    }
    if common::bound_file(backend, &result["closure"], "重启收尾")?
        != run.join("g0070/reboot-close/closure.json")
    {
        return Err("主机重启收尾收据不属于固定代次目录".into());
    }
    let value = common::read_bound(backend, &result["closure"], "重启收尾")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "status",
            "start",
            "request",
            "failed_prefix",
            "reboot",
            "roles",
            "storage",
            "cache",
            "baseline",
            "running",
            "declared_deltas",
            "verification_qualified",
            "replay_allowed",
            "remote_writes",
            "restore_qualified",
        ],
        "重启收尾",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "seed-source-generation-reboot-closure"
        || value["status"] != "seed_source_generation_reboot_closed"
        || value["verification_qualified"] != false
        || value["replay_allowed"] != false
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"] != false
        || value["declared_deltas"]
            != json!({"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0})
    {
        return Err("主机重启收尾收据的状态、计数或声明增量无效".into());
    }
    let generations = document::array(&value["cache"]["generations"], "重启收尾缓存代次")?;
    if generations.is_empty() || generations.iter().any(|row| row["state"] != "stopped") {
        return Err("主机重启收尾收据没有记录已完成退出的缓存代次".into());
    }
    Ok(json!({"binding":result["closure"],"receipt":value}))
}

fn closed_ports(request: &Value, contract: &Value, boot: u64) -> Result<()> {
    let execution = Path::new(document::text(
        &request["execution_backend"],
        "重启源执行根",
    )?);
    let config = Path::new(document::text(
        &contract["api_config_dir"],
        "重启源冻结配置目录",
    )?);
    let table = crate::rate_limit::load_app_table(execution, Some(config), "test")
        .map_err(|_| "重启源配置无法读取；检查冻结配置目录")?;
    let app = table
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
        .filter(|p| (1..=65535).contains(p))
        .ok_or("重启源配置 app.port 无效")?;
    common::require_closed_port(&format!("http://{host}:{port}"))?;
    common::require_closed_port(document::text(
        &contract["worker_ready_url"],
        "重启源 Worker 地址",
    )?)?;
    host::exited(&request["current_storage"]["storage"]["identity"], boot)?;
    for key in ["api_url", "console_url"] {
        common::require_closed_port(document::text(
            &request["current_storage"][key],
            "重启源旧存储端点",
        )?)?;
    }
    Ok(())
}

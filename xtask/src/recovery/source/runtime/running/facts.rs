//! 完整 START 事实只由原账本、历史证明和实际运行观察派生，不接受跨进程事实列表。
use super::super::{images, lineage, staging};
use super::history;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        json as document,
        lifecycle::state,
        resources::{common, storage::registered},
        seed::{generation::coordinator, history::post, source_registration::copied},
        source,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::source::evidence) fn load(
    backend: &Path,
    start: &Value,
    live: bool,
    historical: bool,
) -> Result<Value> {
    let path = common::bound_file(backend, start, "来源 START")?;
    let run = path
        .parent()
        .filter(|p| p.file_name().is_some_and(|name| name == "results"))
        .and_then(Path::parent)
        .ok_or("来源 START 必须属于原账本 results")?;
    common::local_directory(backend, run, "来源原账本")?;
    let state_binding = common::binding(&run.join("state.json"))?;
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let selected = model(
        "select-start",
        json!({"attempts":history::starts(backend,run,attempts)?}),
    )?;
    let record = &selected["record"];
    let number = record["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("来源 START 序号无效")?;
    if record["result"] != *start || path != run.join(format!("results/{number:04}.json")) {
        return Err("来源验证必须绑定同一账本唯一已通过 START 的外层结果".into());
    }
    let coordinator = record["sources"].clone();
    coordinator_source(backend, &coordinator, historical)?;
    let value = common::read_bound(backend, start, "来源 START")?;
    let output = run.join(format!("g{number:04}"));
    let source = history::predecessor(backend, run, &saved)?;
    let request = common::read_bound(backend, &value["request"], "来源运行请求")?;
    let prefix = attempts
        .iter()
        .take_while(|row| row["number"] != record["number"])
        .cloned()
        .collect::<Vec<_>>();
    let source_model = json!({"review_successor":source["review_successor"],"source_rebind":source["source_rebind"],
        "review_successor_binding":source["review_successor_binding"],"storage":source["storage"],"source_generation":source.get("source_generation")});
    model(
        "running",
        json!({"receipt":value,"request":request,"source":source_model,"prefix":prefix,
        "coordinator_source":coordinator,"generation_id":output.file_name().and_then(|s|s.to_str()).ok_or("来源代次名称无效")?,
        "request_copy":document::read(&output.join("request.json"))?,"attempts":attempts,"number":number,"live":live}),
    )?;
    fixed_paths(backend, &output, &value)?;
    let (selected, before, running) = complete_images(backend, &output, &source, &value)?;
    let lineage = dataset(backend, run, &source, &value, &before)?;
    let runtime = super::runtime(
        backend,
        &output,
        &request,
        &selected,
        &source["generation"]["physical_binding"],
        live,
    )?;
    if live
        && serde_json::to_value(registered::current(backend, run, CloneSide::Target)?)?
            != value["current_storage"]
    {
        return Err("来源运行中的实际存储代次不同于 START".into());
    }
    if common::binding(&run.join("state.json"))? != state_binding
        || common::binding(&path)? != *start
    {
        return Err("来源核验期间完整账本或 START 字节变化".into());
    }
    coordinator_source(backend, &coordinator, historical)?;
    Ok(
        json!({"receipt":value,"start_descriptor":start,"number":number,"directory":run,"output":output,
        "request":request,"source":source,"execution":request["execution_backend"],"selected":selected,"runtime":runtime,
        "before":before,"running":running,"lineage":lineage,"coordinator_source":coordinator,
        "environment":common::read_bound(backend,&request["source_environment"],"来源私有环境")?}),
    )
}

fn fixed_paths(backend: &Path, output: &Path, receipt: &Value) -> Result<()> {
    for (field, name) in [
        ("intent", "intent.json"),
        ("runtime_evidence", "running-evidence.json"),
        ("before", "before/image.json"),
        ("running", "running/image.json"),
        ("dataset_lineage", "dataset-lineage.json"),
    ] {
        if common::bound_file(backend, &receipt[field], "同代来源证据")? != output.join(name)
        {
            return Err("来源运行证据必须属于同一固定 generation".into());
        }
    }
    Ok(())
}

fn dataset(
    backend: &Path,
    run: &Path,
    source: &Value,
    receipt: &Value,
    before: &Value,
) -> Result<Value> {
    let post = post(
        backend,
        run,
        Some(&source["registration"]["post_copy"]),
        None,
        false,
    )?;
    let plan = copied::require(backend, run, &source["manifest"])?;
    let projection = json!({"review_successor":source["review_successor"],"result":source["result"],
        "registration":source["registration"],"manifest":source["manifest"],"request":source["request"],"directory":run});
    lineage::verify(
        &json!({"backend":backend,"source":projection,"post_request":post.request,
        "post_descriptor":post.descriptor,"copy_plan":plan,"image_descriptor":receipt["before"],
        "verified_image":before,"lineage_descriptor":receipt["dataset_lineage"]}),
    )
}

pub(super) fn coordinator_source(backend: &Path, value: &Value, historical: bool) -> Result<()> {
    staging::validate_source(value)?;
    if !historical && source::current(backend)? != *value {
        return Err("来源协调器必须保持 START 登记的干净 SHA 与完整工具来源".into());
    }
    Ok(())
}

pub(super) fn model(action: &str, input: Value) -> Result<Value> {
    let value = coordinator::dispatch(&json!({"action":action,"input":input}))?;
    if let Some(reason) = value.get("rejected") {
        return Err(document::text(reason, "来源生成模型拒绝原因")?
            .to_owned()
            .into());
    }
    Ok(value)
}

fn complete_images(
    backend: &Path,
    output: &Path,
    source: &Value,
    value: &Value,
) -> Result<(Value, Value, Value)> {
    let mut selected = source["request"]["source"].clone();
    document::object(&selected, "来源完整物理范围")?;
    selected["runtime_dir"] = json!(output.join("runtime"));
    let before = images::verify(
        backend,
        &value["before"],
        &selected,
        &source["request"],
        &value["source_registration"],
    )?;
    let running = images::verify(
        backend,
        &value["running"],
        &selected,
        &source["request"],
        &value["source_registration"],
    )?;
    if before["image"] != running["image"] {
        return Err("来源启动前后完整数据库、对象或 Redis 像变化".into());
    }
    Ok((selected, before, running))
}

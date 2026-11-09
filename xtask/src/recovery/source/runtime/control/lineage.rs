//! C69 只读取 C68 完整前像并采集当前后像；不创建任何启动意图或产品进程。
use super::{
    configuration,
    frame::Frame,
    observation::{self, Observation},
};
use crate::{
    Result,
    recovery::{
        clone::native::{
            digest, export::transport::Transport, json as document, resources::common,
        },
        source::evidence::{
            inventory,
            running::history::lineage::{self, failure},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn prepare(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    request: &Path,
) -> Result<(Value, observation::Prepared)> {
    lineage::model("authorize-stage", json!({"number":number,"prefix":prefix}))?;
    let facts = failure::inspect(backend, run, prefix, Some(request))?;
    let output = run.join("seed-runtime/attempt-0069");
    let config = Path::new(document::text(
        &facts["contract"]["api_config_dir"],
        "C68 绑定 API 配置",
    )?);
    let prepared = observation::prepare(
        backend,
        run,
        number,
        &facts["request_descriptor"],
        &facts["source"],
        &output,
        config,
    )?;
    Ok((facts, prepared))
}

pub(super) fn run<T: Transport + Sync>(
    frame: &Frame<'_>,
    request: &Path,
    transport: &T,
) -> Result<Value> {
    let (facts, prepared) = prepare(
        frame.backend,
        frame.run,
        &frame.prefix,
        frame.number,
        request,
    )?;
    lineage::model(
        "authorize-product",
        json!({"coordinator":frame.source(),"failed":facts["failed"]}),
    )?;
    require_original_artifacts(frame, &facts, &prepared)?;
    let api = Path::new(document::text(
        &facts["contract"]["api_config_dir"],
        "C68 API 配置",
    )?);
    let worker = Path::new(document::text(
        &facts["contract"]["worker_config_dir"],
        "C68 Worker 配置",
    )?);
    let observation = Observation::create(
        frame,
        prepared,
        configuration::Selection::Bound { api, worker },
        transport,
    )?;
    if observation.contract != facts["contract"] {
        return Err("C69 观察运行配置与 C68 不同".into());
    }
    for address in [
        format!(
            "{}/readyz",
            document::text(
                &observation.prepared.facts["selected"]["api_url"],
                "C69 API"
            )?
            .trim_end_matches('/')
        ),
        document::text(&observation.contract["worker_ready_url"], "C69 Worker")?.to_owned(),
    ] {
        crate::recovery::clone::native::target_verify::authority::require_closed(&address)?;
    }
    observation.checkpoint()?;
    let (after, verified) = observation.capture("after")?;
    if verified["image"] != facts["image"] {
        return Err("C69 当前完整像与 C68 启动前像不同，禁止重试".into());
    }
    observation.checkpoint()?;
    let repeated = failure::inspect(frame.backend, frame.run, &frame.prefix, Some(request))?;
    if repeated["proof"] != facts["proof"] || repeated["image"] != facts["image"] {
        return Err("C69 观察期间 C68 失败或完整前像变化".into());
    }
    failure::inputs::exact_directory(
        frame.backend,
        observation.output()?,
        &[],
        &["runtime", "after"],
    )?;
    failure::inputs::exact_directory(
        frame.backend,
        &observation.output()?.join("runtime"),
        &["binaries.json", "runtime.json"],
        &[],
    )?;
    let manifest = manifest(observation.output()?)?;
    observation.checkpoint()?;
    Ok(lineage::model("receipt",json!({"facts":{"request":facts["request"],"request_descriptor":facts["request_descriptor"],
        "proof":facts["proof"],"before":facts["before"]},"prefix":frame.prefix,"coordinator":frame.source(),
        "runtime":common::binding(&observation.output()?.join("runtime/runtime.json"))?,"after":after,"manifest":manifest}))?["receipt"].clone())
}

fn manifest(output: &Path) -> Result<Value> {
    let files = inventory::manifest(output)?;
    let bytes = files.iter().try_fold(0_u64, |sum, row| {
        sum.checked_add(row["bytes"].as_u64().ok_or("谱系目录文件大小无效")?)
            .ok_or("谱系目录总字节溢出")
    })?;
    Ok(json!({"files":files.len(),"bytes":bytes,"sha256":digest::plan_hash(&json!(files))?}))
}

fn require_original_artifacts(
    frame: &Frame<'_>,
    facts: &Value,
    prepared: &observation::Prepared,
) -> Result<()> {
    let historical = if facts["historical_request"] != facts["request"] {
        failure::inputs::binaries(frame.backend, &facts["historical_request"])?
    } else {
        prepared.binaries.clone()
    };
    let old_runtime = Path::new(document::text(
        &facts["proof"]["runtime"]["path"],
        "C68 原 runtime",
    )?);
    if document::read(
        &old_runtime
            .parent()
            .ok_or("C68 runtime 缺少父目录")?
            .join("binaries.json"),
    )? != historical
    {
        return Err("C69 观察运行使用的产品或维护二进制与 C68 不同".into());
    }
    Ok(())
}

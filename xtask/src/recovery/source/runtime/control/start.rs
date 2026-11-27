//! START 在统一持锁区间消费双角色预留；未知结果保留原意图和完整前后像。
use super::{
    configuration,
    frame::Frame,
    observation::{self, Observation, Prepared},
    trees,
};
use crate::{
    Result,
    recovery::{
        clone::native::{
            export::transport::Transport, json as document, lifecycle::state, resources::common,
            seed::generation::coordinator,
        },
        full_stack::worker::{role::Role, start as supervised},
        source::evidence::running::history,
        source::evidence::running::roles,
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "start/publication.rs"]
mod publication;

pub(super) fn run<T: Transport + Sync>(
    frame: &mut Frame<'_>,
    request: &Path,
    transport: &T,
) -> Result<Value> {
    let plan = Plan::load(frame, request)?;
    #[cfg(windows)]
    frame.hold_sources(&plan.prepared.facts["request"])?;
    let archive = plan.archive;
    let segment = plan.segment;
    let config = plan.prepared.config.clone();
    let observation = Observation::create(
        frame,
        plan.prepared,
        configuration::Selection::Copy(&config),
        transport,
    )?;
    let output = observation.output()?;
    document::write_new(
        &output.join("request.json"),
        &observation.prepared.facts["request"],
    )?;
    observation.quiet()?;
    let execution = Path::new(document::text(
        &observation.prepared.facts["execution"],
        "START执行根",
    )?);
    let runtime = output.join("runtime");
    let environment = &observation.prepared.facts["request"]["source_environment"];
    let api = supervised::reserve(Role::Api, execution, &runtime, environment)?;
    let worker = supervised::reserve(Role::Worker, execution, &runtime, environment)?;
    let operations = json!({"api":api.id(),"worker":worker.id()});
    document::write_new(
        &output.join("intent.json"),
        &json!({"format_version":1,"kind":"seed-source-runtime-intent",
        "request":common::binding(&output.join("request.json"))?,"runtime_directory":runtime,"operations":operations}),
    )?;
    let guard = crate::recovery::clone::native::target_verify::guard::Inputs::open([
        output.join("request.json"),
        output.join("intent.json"),
    ])?;
    let checkpoint = || {
        guard.check()?;
        observation.checkpoint()?;
        guard.check()
    };
    checkpoint()?;
    let (before, image) = observation.capture("before")?;
    archived_image(&archive, &segment, &image["image"])?;
    let lineage = publication::lineage(&observation, &before, &image)?;
    document::write_new(&output.join("dataset-lineage.json"), &lineage)?;
    let result = (|| {
        checkpoint()?;
        api.start(90.0)?;
        checkpoint()?;
        worker.start(90.0)?;
        checkpoint()?;
        let (running, current) = observation.capture("running")?;
        if image["image"] != current["image"] {
            return Err("源启动前后完整数据库、对象或Redis像变化；保留现场，不发布、不重放".into());
        }
        publication::evidence(&observation, &operations)?;
        checkpoint()?;
        let receipt = publication::receipt(&observation, &before, &running)?;
        checkpoint()?;
        Ok(receipt)
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => failure(&observation, &image["image"], error),
    }
}

fn failure<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    before: &Value,
    failure: Box<dyn std::error::Error>,
) -> Result<Value> {
    let mut facts = observation.prepared.facts.clone();
    facts["runtime"] = observation.contract.clone();
    let cleanup: Result<()> = (|| {
        trees::stop(observation.frame, &facts)?;
        observation.checkpoint()?;
        let (_, after) = observation.capture("after")?;
        if after["image"] != *before {
            return Err("失败启动后完整像变化；禁止重放".into());
        }
        Ok(())
    })();
    match cleanup {
        Ok(()) => Err(failure),
        Err(error) => Err(format!(
            "源启动失败：{failure}；原监督树回收或失败后像未确认：{error}；保留全部证据，不重放"
        )
        .into()),
    }
}

fn archived_image(archive: &Value, segment: &Value, current: &Value) -> Result<()> {
    if archive.is_null() {
        return Ok(());
    }
    if !segment["reboot"].is_null() {
        let mut expected = archive["image"].clone();
        expected["storage"] =
            json!({"rustfs":segment["storage"]["storage"],"redis":segment["cache"]["redis"]});
        crate::recovery::source::evidence::model::write_effects(
            &json!({"before":expected,"after":current}),
        )?;
        return Ok(());
    }
    // 复用原算法；只需提升其可见性，不复制比较或增加文件桥。
    crate::recovery::clone::native::seed::generation::archived_image(
        &archive["image"],
        current,
        segment,
    )
}

pub(super) struct Plan {
    pub prepared: Prepared,
    pub archive: Value,
    pub segment: Value,
}

pub(super) fn preflight(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    request: &Path,
) -> Result<(Value, Value)> {
    let descriptor = common::binding(request)?;
    let value = common::read_bound(backend, &descriptor, "正式 START 请求")?;
    coordinator::current_request(&value)?;
    let archive = history::prelaunch::closed(backend, run, attempts)?;
    partial_request(&archive, &descriptor)?;
    let planned = model(
        "preflight-plan",
        json!({"attempts":attempts,"archive":archive}),
    )?;
    let segment = if document::boolean(&planned["needs_segment"], "START 历史续作选择")? {
        history::segment::read(
            backend,
            run,
            attempts,
            &archive,
            &archive["receipt"]["source_registration"],
            &Value::Null,
        )?
    } else {
        Value::Null
    };
    model(
        "preflight-finish",
        json!({"attempts":attempts,"archive":archive,
        "segment":segment,"request":descriptor,"current_request":common::binding(request)?}),
    )?;
    common::bound_file(backend, &descriptor, "正式 START 请求")?;
    Ok((archive, segment))
}

fn partial_request(archive: &Value, descriptor: &Value) -> Result<()> {
    let receipt = &archive["receipt"];
    if (receipt["status"] == "seed_source_partial_prelaunch_closed"
        || receipt["status"] == super::super::running::history::prelaunch::revalidated::STATUS)
        && receipt["request"] != *descriptor
    {
        return Err("部分启动前封存后的实际 START 必须使用同一正式请求".into());
    }
    Ok(())
}

impl Plan {
    pub(super) fn load(frame: &Frame<'_>, request: &Path) -> Result<Self> {
        if frame.mode != "source-generation-start" || frame.source()["snapshot"]["clean"] != true {
            return Err("START 必须由当前干净来源的正式持锁阶段执行".into());
        }
        frame.checkpoint()?;
        let (archive, segment) = preflight(frame.backend, frame.run, &frame.prefix, request)?;
        let descriptor = common::binding(request)?;
        let value = common::read_bound(frame.backend, &descriptor, "正式 START 请求")?;
        let source = history::predecessor(frame.backend, frame.run, frame.state())?;
        let checked = model(
            "inputs",
            json!({"request":value,"source":source,
            "attempts":state::attempts(frame.state())?,"number":frame.number}),
        )?;
        if checked["prefix"] != json!(frame.prefix) {
            return Err("START 输入模型没有绑定完整持锁历史前缀".into());
        }
        let prepared = observation::prepare(
            frame.backend,
            frame.run,
            frame.number,
            &descriptor,
            &source,
            &frame.run.join(format!("g{:04}", frame.number)),
            &crate::recovery::source::evidence::generation_request::configuration::directory(
                frame.backend,
                &value,
            )?,
        )?;
        if !archive.is_null() {
            let previous = common::read_bound(
                frame.backend,
                &archive["receipt"]["request"],
                "封存原 START 请求",
            )?;
            let selected = if segment.is_null() {
                Value::Null
            } else {
                json!({"phase":segment["phase"],"storage":segment["storage"],"cache":segment["cache"]})
            };
            if crate::recovery::clone::native::seed::generation::archived_request(
                &previous, &value, &selected,
            )? {
                crate::recovery::clone::native::seed::generation::builds::verify(
                    frame.backend,
                    &previous,
                    &value,
                )?;
            }
        }
        common::bound_file(frame.backend, &descriptor, "正式 START 请求")?;
        frame.checkpoint()?;
        Ok(Self {
            prepared,
            archive,
            segment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_prelaunch_start_requires_original_request_binding() {
        let request = json!({"path":"original.json","bytes":12,"sha256":"digest"});
        let mut other = request.clone();
        other["path"] = json!("identical-copy.json");
        let mut archive = json!({"receipt":{"request":request}});
        for status in [
            "seed_source_partial_prelaunch_closed",
            super::super::super::running::history::prelaunch::revalidated::STATUS,
        ] {
            archive["receipt"]["status"] = json!(status);
            assert!(partial_request(&archive, &request).is_ok());
            assert!(partial_request(&archive, &other).is_err());
        }
        archive["receipt"]["status"] = json!("seed_source_prelaunch_closed");
        assert!(partial_request(&archive, &other).is_ok());
    }
}

fn model(action: &str, input: Value) -> Result<Value> {
    let value = coordinator::dispatch(&json!({"action":action,"input":input}))?;
    if let Some(reason) = value.get("rejected") {
        return Err(document::text(reason, "START 阶段拒绝原因")?
            .to_owned()
            .into());
    }
    Ok(value)
}

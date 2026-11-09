//! 两次完整观察共享原生来源、历史与资源证明；任一漂移都不能产生成功收据。
use super::{Controller, Phase, classify};
use crate::{
    Result,
    cli::CloneSide,
    recovery::{
        clone::native::{
            cache, json as document,
            resources::{common, storage::registered},
            seed::protocol::guards,
        },
        source::evidence,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn collect(controller: &Controller<'_>) -> Result<Value> {
    let before = inspect(controller)?;
    let after = inspect(controller)?;
    classify(
        Phase::Comparison,
        model(
            "register-receipt",
            json!({"before":before,"after":after,"request":controller.request()}),
        ),
    )
}

fn inspect(controller: &Controller<'_>) -> Result<Value> {
    controller.checked(|| {
        let backend = controller.backend();
        let run = controller.run();
        let source = classify(
            Phase::Source,
            cache::binding::source::load(backend, controller.request()),
        )?;
        if source["directory"] != json!(run) {
            return classify(
                Phase::Source,
                Err("source-rebind 请求的 C52 不属于当前 run".into()),
            );
        }
        let descriptor = &source["review_successor"]["source_result"];
        let history = evidence::source_rebind_history(controller, descriptor)?;
        let planned = classify(
            Phase::History,
            model(
                "register-plan",
                json!({"later":history["later"],"number":controller.number()}),
            ),
        )?;
        classify(
            Phase::Cache,
            cache_successor(controller, &planned, descriptor),
        )?;
        classify(
            Phase::Quiet,
            guards::rebind::live::quiet(
                backend,
                &json!({"registration":source["registration"],"request":source["request"]}),
            ),
        )?;
        let current = classify(
            Phase::Storage,
            registered::current(backend, run, CloneSide::Target),
        )?
        .unwrap_or(Value::Null);
        classify(
            Phase::Transition,
            transition(backend, &source["storage"]["storage"], &current),
        )?;
        let rows = document::array(&controller.saved()["attempts"], "重绑定完整历史")?;
        Ok(json!({"source":source,"current":current,"prefix":&rows[..rows.len()-1]}))
    })
}

fn cache_successor(controller: &Controller<'_>, planned: &Value, descriptor: &Value) -> Result<()> {
    if planned["cache"].is_null() {
        return Ok(());
    }
    let successor = cache::binding::execute_stage(
        controller.backend(),
        &json!({"action":"history-proof","directory":controller.run(),"caller":std::process::id(),
            "parameters":{"attempt":planned["cache"],"source_result":descriptor,"frozen_successor":null}}),
        None,
    )?;
    if successor != *controller.request() {
        return Err("source-rebind 必须消费同一缓存工具 successor".into());
    }
    Ok(())
}

fn transition(backend: &Path, original: &Value, current: &Value) -> Result<()> {
    common::directory_identity(backend, &original["data_directory"])?;
    guards::rebind::live::transition(backend, &json!({"original":original,"current":current}))?;
    common::directory_identity(backend, &original["data_directory"])?;
    Ok(())
}

pub(super) fn model(action: &str, input: Value) -> Result<Value> {
    let value = guards::rebind::dispatch(&json!({"action":action,"input":input}))?;
    if let Some(message) = value.get("rejected") {
        return Err(document::text(message, "重绑定拒绝原因")?.to_owned().into());
    }
    Ok(value)
}

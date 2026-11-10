//! 运行收据的唯一原生只读组合；完整事实、生产者、审计、缓存及双像逐项复核。
use super::super::{
    environment, images, model, producer, ready, running, session::policy, staging,
};
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, source},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    descriptor: &Value,
    live: bool,
) -> Result<Value> {
    let path = common::bound_file(backend, descriptor, "来源运行收据")?;
    let directory = path
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "verification"))
        .ok_or("来源运行收据必须属于同代 verification")?;
    if path != directory.join("source-runtime.json") {
        return Err("来源运行收据文件名无效".into());
    }
    let receipt = common::read_bound(backend, descriptor, "来源运行收据")?;
    let recovered = policy(
        "runtime-schema",
        json!({"backend":backend,"directory":directory,"value":receipt}),
    )?["recovered"]
        .as_bool()
        .ok_or("来源运行收据恢复选择无效")?;
    let selection = policy(
        "runtime-facts",
        json!({"backend":backend,"receipt":receipt}),
    )?;
    if recovered && live && source::current(backend)? != selection["successor"] {
        return Err("来源运行恢复工具与已绑定的干净后继不同".into());
    }
    let facts = running::load(backend, &receipt["source_generation"], live, recovered)?;
    policy(
        "facts-binding",
        json!({"directory":directory,"generation":facts["output"],
        "receipt":receipt,"lineage":facts["receipt"]["dataset_lineage"]}),
    )?;
    let node = producer(backend, directory, &facts, recovered)?;
    let verified = model::result(&json!({"value":node["stdout"],"lineage":facts["lineage"],
        "start":receipt["source_generation"],"lineage_descriptor":receipt["dataset_lineage"]}))?;
    policy(
        "login-stored",
        json!({"backend":backend,"directory":directory,"descriptor":receipt["login_audit"],
        "lineage":facts["lineage"],"subjects":verified["subjects"],"started_at":receipt["started_at"]}),
    )?;
    let cleanup = cleanup(
        backend,
        directory,
        &receipt["cache_cleanup"],
        &verified["subjects"],
    )?;
    let before = images::verify(
        backend,
        &receipt["before"],
        &facts["selected"],
        &facts["source"]["request"],
        &facts["receipt"]["source_registration"],
    )?;
    let after = images::verify(
        backend,
        &receipt["after"],
        &facts["selected"],
        &facts["source"]["request"],
        &facts["receipt"]["source_registration"],
    )?;
    let effects = model::write_effects(&json!({"before":before["image"],"after":after["image"]}))?;
    if recovered {
        let chain = policy(
            "recovery-chain",
            json!({"backend":backend,"directory":directory,
            "descriptor":receipt["recovery"],"receipt":receipt,"cleanup":cleanup}),
        )?;
        staging::validate_source(&chain["successor"])?;
    }
    policy(
        "runtime-final",
        json!({"backend":backend,"directory":directory,"descriptor":descriptor,
        "receipt":receipt,"before":before["image"],"expected_before":facts["running"]["image"],
        "effects":effects,"cleanup":cleanup,"recovered":recovered}),
    )?;
    Ok(
        json!({"receipt":receipt,"before":before,"after":after,"producer":node["producer"],"lineage":facts["lineage"],"facts":facts}),
    )
}

fn producer(backend: &Path, directory: &Path, facts: &Value, historical: bool) -> Result<Value> {
    let private: BTreeMap<String, String> =
        serde_json::from_value(facts["environment"]["environment"].clone())?;
    let environment = environment::configured(&private)?;
    let tool = crate::rate_limit::current_tool_binding()?;
    producer::Inputs {
        backend,
        execution: Path::new(document::text(&facts["execution"], "来源执行树")?),
        directory,
        start: &facts["start_descriptor"],
        lineage: &facts["receipt"]["dataset_lineage"],
        environment: &environment,
        tool: Path::new(document::text(&tool["path"], "原生协调工具")?),
        coordinator: &facts["coordinator_source"],
        execution_sha: document::text(&facts["request"]["expected_backend_sha"], "来源执行 SHA")?,
        historical,
    }
    .verify()
}

fn cleanup(
    backend: &Path,
    directory: &Path,
    descriptor: &Value,
    subjects: &Value,
) -> Result<Value> {
    let path = common::bound_file(backend, descriptor, "来源缓存清理收据")?;
    if path != directory.join("cache-cleanup.json") {
        return Err("来源缓存清理必须属于同代 verification".into());
    }
    let value = common::read_bound(backend, descriptor, "来源缓存清理收据")?;
    ready::policy::cache_attempt::receipt(
        &json!({"backend":backend,"directory":directory,"value":value,"subjects":subjects}),
    )
}

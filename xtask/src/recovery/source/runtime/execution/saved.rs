//! 运行收据的唯一原生只读组合；完整事实、生产者、审计、缓存及双像逐项复核。
use super::super::{
    environment, images, model, producer, ready, running, session::policy, staging,
};
use super::warm;
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, source},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
enum ProducerTool {
    Current,
    Published,
}

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    descriptor: &Value,
    live: bool,
) -> Result<Value> {
    verify_with_tool(backend, descriptor, live, ProducerTool::Current)
}

/// 由已发布 STOP 的读取方调用，以同代原生产者为准复核全部已封存证据。
pub(in crate::recovery::source::evidence) fn verify_published(
    backend: &Path,
    descriptor: &Value,
) -> Result<Value> {
    verify_with_tool(backend, descriptor, false, ProducerTool::Published)
}

fn verify_with_tool(
    backend: &Path,
    descriptor: &Value,
    live: bool,
    mode: ProducerTool,
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
    let trusted_warm = warm::mode(&facts)?;
    if trusted_warm != (receipt["format_version"].as_u64() == Some(4)) || trusted_warm && recovered
    {
        return Err("来源核验收据版本与已登记的 Redis 模式不同".into());
    }
    policy(
        "facts-binding",
        json!({"directory":directory,"generation":facts["output"],
        "receipt":receipt,"lineage":facts["receipt"]["dataset_lineage"]}),
    )?;
    let intent = &receipt["producer"]["intent"];
    let tool = match mode {
        ProducerTool::Current => PathBuf::from(document::text(
            &crate::rate_limit::current_tool_binding()?["path"],
            "原生协调工具",
        )?),
        ProducerTool::Published => published_tool(backend, directory, intent)?,
    };
    let node = producer(backend, directory, &facts, recovered, &tool)?;
    let verified = model::result_with_sessions(
        &json!({"value":node["stdout"],"lineage":facts["lineage"],
        "start":receipt["source_generation"],"lineage_descriptor":receipt["dataset_lineage"]}),
        trusted_warm,
    )?;
    policy(
        "login-stored",
        json!({"backend":backend,"directory":directory,"descriptor":receipt["login_audit"],
        "lineage":facts["lineage"],"subjects":verified["subjects"],"started_at":receipt["started_at"]}),
    )?;
    let verify_image = |key: &str| {
        images::verify(
            backend,
            &receipt[key],
            &facts["selected"],
            &facts["source"]["request"],
            &facts["receipt"]["source_registration"],
        )
    };
    let before = verify_image("before")?;
    let after = verify_image("after")?;
    let (cleanup, effects) = checked_effects(
        backend,
        directory,
        &receipt,
        &facts,
        &verified,
        [&before["image"], &after["image"]],
        trusted_warm,
    )?;
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
    if matches!(mode, ProducerTool::Published) {
        common::bound_file(backend, intent, "已发布来源原生产者意图")?;
    }
    Ok(
        json!({"receipt":receipt,"before":before,"after":after,"producer":node["producer"],"lineage":facts["lineage"],"facts":facts}),
    )
}

fn checked_effects(
    backend: &Path,
    directory: &Path,
    receipt: &Value,
    facts: &Value,
    verified: &Value,
    [before, after]: [&Value; 2],
    trusted_warm: bool,
) -> Result<(Value, Value)> {
    if trusted_warm {
        let cleanup = warm::observation(backend, directory, receipt, before, facts, verified)?;
        let effects = warm::effects(
            backend,
            before,
            after,
            verified,
            &receipt["producer"],
            &cleanup,
        )?;
        return Ok((cleanup, effects));
    }
    let cleanup = cleanup(
        backend,
        directory,
        &receipt["cache_cleanup"],
        &verified["subjects"],
    )?;
    let effects = model::write_effects(&json!({"before":before,"after":after}))?;
    Ok((cleanup, effects))
}

fn published_tool(backend: &Path, directory: &Path, binding: &Value) -> Result<PathBuf> {
    let path = common::bound_file(backend, binding, "已发布来源原生产者意图")?;
    if path != directory.join("source-producer-intent.json") {
        return Err("已发布来源原生产者意图必须属于同一 verification".into());
    }
    let intent = document::read(&path)?;
    let executable = PathBuf::from(document::text(
        &intent["executable"],
        "已发布来源原生产者程序",
    )?);
    if intent["format_version"].as_u64() != Some(2)
        || intent["kind"] != "restore-source-producer-intent"
        || !executable.is_absolute()
        || common::binding(&path)? != *binding
    {
        return Err("已发布来源原生产者意图或程序绑定无效".into());
    }
    Ok(executable)
}

fn producer(
    backend: &Path,
    directory: &Path,
    facts: &Value,
    historical: bool,
    tool: &Path,
) -> Result<Value> {
    let private: BTreeMap<String, String> =
        serde_json::from_value(facts["environment"]["environment"].clone())?;
    let environment = environment::configured(&private)?;
    producer::Inputs {
        backend,
        execution: Path::new(document::text(&facts["execution"], "来源执行树")?),
        directory,
        start: &facts["start_descriptor"],
        lineage: &facts["receipt"]["dataset_lineage"],
        environment: &environment,
        tool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_tool_selects_only_the_bound_same_generation_intent() {
        let fixture = tempfile::tempdir().unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fixture.path().canonicalize().unwrap(),
        );
        let directory = backend.join(".local-tests/g0001/verification");
        let sibling = backend.join(".local-tests/g0002/verification");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let executable = backend.join("frozen/original-xtask.exe");
        let intent = json!({"format_version":2,"kind":"restore-source-producer-intent",
            "executable":executable});
        let file = directory.join("source-producer-intent.json");
        let other = sibling.join("source-producer-intent.json");
        document::write_new(&file, &intent).unwrap();
        std::fs::copy(&file, &other).unwrap();
        let binding = common::binding(&file).unwrap();
        assert_eq!(
            published_tool(&backend, &directory, &binding).unwrap(),
            executable
        );
        let other_binding = common::binding(&other).unwrap();
        assert_eq!(binding["sha256"], other_binding["sha256"]);
        assert!(published_tool(&backend, &directory, &other_binding).is_err());

        let mut changed = intent;
        changed["executable"] = json!("redirected.exe");
        document::write_replace(&file, &changed).unwrap();
        assert!(published_tool(&backend, &directory, &binding).is_err());
        let changed_binding = common::binding(&file).unwrap();
        assert!(published_tool(&backend, &directory, &changed_binding).is_err());
    }
}

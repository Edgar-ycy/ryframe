//! 生产者参数、登记与完成证据的唯一模型；文件绑定不能由调用方自报替代。
use super::{documents, model};
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};
use std::path::Path;

fn operation(value: &Value) -> Result<&str> {
    let text = documents::text(value, "来源生产者操作标识")?;
    if text.len() != 32
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("来源验收生产者 operation ID 无效".into());
    }
    Ok(text)
}

pub(super) fn validate_operation(args: &Value) -> Result<Value> {
    documents::exact(args, &["value"], "来源生产者操作标识参数")?;
    Ok(json!(operation(&args["value"])?))
}

pub(super) fn command(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "staging",
            "backend",
            "execution",
            "directory",
            "operation",
            "tool",
            "start",
            "lineage",
            "configuration",
        ],
        "来源生产者命令参数",
    )?;
    operation(&args["operation"])?;
    let staging = Path::new(documents::text(&args["staging"], "来源staging")?);
    let manifest = staging.join("manifest.json");
    let contract = staging.join("openapi/openapi.json");
    let mut argv = vec![args["tool"].clone(), json!("__recovery-existing-source")];
    for (flag, value) in [
        ("--backend-dir", args["backend"].clone()),
        ("--lineage", args["lineage"]["path"].clone()),
        ("--contract", json!(contract)),
        ("--staging-manifest", json!(manifest)),
        ("--configuration", args["configuration"].clone()),
        ("--configuration-root", args["execution"].clone()),
        ("--run-dir", args["directory"].clone()),
        ("--operation-id", args["operation"].clone()),
        (
            "--source-generation-sha256",
            args["start"]["sha256"].clone(),
        ),
    ] {
        documents::text(&value, "来源生产者参数")?;
        argv.extend([json!(flag), value]);
    }
    argv.push(json!("--write"));
    Ok(json!({"manifest":manifest,"argv":argv}))
}

fn identity(value: &Value) -> Result<()> {
    documents::exact(
        value,
        &["pid", "started", "executable"],
        "来源生产者内核身份",
    )?;
    if value["pid"].as_u64().is_none_or(|pid| pid <= 1)
        || !Path::new(documents::text(
            &value["executable"],
            "来源生产者可执行文件",
        )?)
        .is_absolute()
    {
        return Err("来源验收生产者内核身份无效".into());
    }
    documents::text(&value["started"], "来源生产者创建时刻")?;
    Ok(())
}

pub(super) fn registration(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "start",
            "lineage",
            "tool",
            "argv",
            "environment_sha256",
            "coordinator_source",
            "execution_sha",
            "configuration",
        ],
        "来源生产者登记参数",
    )?;
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let directory = Path::new(documents::text(&args["directory"], "来源生产者目录")?);
    common::local_directory(backend, directory, "来源生产者目录")?;
    let intent_binding = common::binding(&directory.join("source-producer-intent.json"))?;
    let intent = common::read_bound(backend, &intent_binding, "来源验收生产者intent")?;
    documents::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "operation_id",
            "controller",
            "source_generation",
            "dataset_lineage",
            "executable",
            "source",
            "configuration",
            "argv",
            "cwd",
            "environment_sha256",
            "started_at",
            "coordinator_source",
            "execution_sha",
            "staging",
            "inputs",
        ],
        "来源验收生产者 intent",
    )?;
    let configuration = common::bound_file(backend, &intent["configuration"], "来源原生配置绑定")?;
    if configuration != directory.join("source-producer-configuration.json")
        || crate::recovery::clone::native::json::read(&configuration)? != args["configuration"]
    {
        return Err("来源原生配置文件绑定发生变化或不属于同一verification目录".into());
    }
    operation(&intent["operation_id"])?;
    let expected = json!({"format_version":2,"kind":"restore-source-producer-intent",
        "source_generation":args["start"],"dataset_lineage":args["lineage"],"executable":args["tool"],
        "argv":args["argv"],"cwd":args["backend"],"environment_sha256":args["environment_sha256"],
        "coordinator_source":args["coordinator_source"],"execution_sha":args["execution_sha"]});
    if expected
        .as_object()
        .ok_or("生产者期望模型无效")?
        .iter()
        .any(|(key, value)| intent[key] != *value)
    {
        return Err("来源验收生产者 intent 没有绑定同一 start、血缘、源码或环境".into());
    }
    identity(&intent["controller"])?;
    model::time_order(&json!({"label":"生产者开始时间","values":[intent["started_at"]]}))?;
    let producer = common::read_bound(
        backend,
        &common::binding(&directory.join("source-producer.json"))?,
        "来源验收生产者",
    )?;
    identity(&producer["process"])?;
    if producer["format_version"].as_u64() != Some(1)
        || producer
            != json!({
        "format_version":1,"kind":"restore-source-producer","operation_id":intent["operation_id"],
        "intent":intent_binding,"controller":intent["controller"],"process":producer["process"],
        "executable":args["tool"],"argv":args["argv"]})
    {
        return Err("来源验收生产者登记与当前输入不同".into());
    }
    Ok(json!({"intent":intent,"producer":producer}))
}

pub(super) fn completion(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "intent", "producer"],
        "来源生产者完成参数",
    )?;
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let directory = Path::new(documents::text(&args["directory"], "来源生产者目录")?);
    common::local_directory(backend, directory, "来源生产者目录")?;
    let value = common::read_bound(
        backend,
        &common::binding(&directory.join("source-producer-completion.json"))?,
        "来源验收生产者completion",
    )?;
    let mut expected = json!({"format_version":1,"kind":"restore-source-producer-completion",
        "operation_id":args["intent"]["operation_id"],"process":args["producer"]["process"],
        "exit_code":value["exit_code"],"completed_at":value["completed_at"]});
    for (key, file) in [
        ("intent", "source-producer-intent.json"),
        ("producer", "source-producer.json"),
        ("ready", "source-producer-ready.json"),
        ("stdout", "source-producer.stdout.json"),
        ("stderr", "source-producer.stderr.log"),
    ] {
        expected[key] = common::binding(&directory.join(file))?;
    }
    if value["format_version"].as_u64() != Some(1)
        || value["exit_code"].as_i64().is_none()
        || value != expected
    {
        return Err("来源验收生产者完成证据或退出状态不可信".into());
    }
    model::time_order(
        &json!({"label":"来源生产者完成时间","values":[args["intent"]["started_at"],value["completed_at"]]}),
    )?;
    Ok(value)
}

//! 在启动前持久化精确 intent；内核身份登记前不向生产者提供 stdin 授权。
use super::super::{inventory, staging};
use super::Inputs;
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common},
};
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write, path::Path};

pub(in crate::recovery::source::evidence) const FILES: &[&str] = &[
    "source-producer-intent.json",
    "source-producer.json",
    "source-producer-ready.json",
    "source-producer.stdout.json",
    "source-producer.stderr.log",
    "source-producer-completion.json",
    "source-producer-configuration.json",
];

pub(super) fn prepare(inputs: &Inputs<'_>, operation: &str) -> Result<Value> {
    inputs.policy("producer-operation", json!({"value":operation}))?;
    common::local_directory(inputs.backend, inputs.directory, "来源生产者 verification")?;
    if inputs.historical
        || inputs
            .directory
            .file_name()
            .is_none_or(|name| name != "verification")
    {
        return Err("新的来源业务生产者只能在当前验证的 verification 目录启动".into());
    }
    for name in FILES {
        if inputs.directory.join(name).try_exists()? {
            return Err("来源验收已有生产者文件；保留现场，禁止重放启动".into());
        }
    }
    common::bound_file(inputs.backend, inputs.start, "来源 START")?;
    common::bound_file(inputs.backend, inputs.lineage, "来源 dataset 血缘")?;
    inputs.source_unchanged()?;
    let controller =
        common::process_identity(std::process::id())?.ok_or("来源控制器缺少内核创建身份")?;
    let tool = common::binding(inputs.tool)?;
    let configuration_path = inputs.directory.join("source-producer-configuration.json");
    let (_, configuration) =
        super::super::generation_request::configuration::for_start(inputs.backend, inputs.start)?;
    document::write_new(&configuration_path, &configuration)?;
    let descriptor = staging::create(
        &json!({"coordinator":inputs.backend,"execution":inputs.execution,
        "directory":inputs.directory,"coordinator_source":inputs.coordinator,"execution_sha":inputs.execution_sha,
        "tool":{"path":inputs.tool,"sha256":tool["sha256"]}}),
    )?;
    let staged = staging::verify(
        &json!({"coordinator":inputs.backend,"execution":inputs.execution,
        "directory":inputs.directory,"coordinator_source":inputs.coordinator,"execution_sha":inputs.execution_sha,
        "tool":{"path":inputs.tool,"sha256":tool["sha256"]},"descriptor":descriptor}),
    )?;
    let command = inputs.policy("producer-command", json!({"staging":staged["root"],"backend":inputs.backend,
        "execution":inputs.execution,"directory":inputs.directory,"operation":operation,"tool":inputs.tool,
        "start":inputs.start,"lineage":inputs.lineage,"configuration":configuration_path}))?;
    let files =
        inventory::files_request(&json!({"backend":inputs.backend,"directory":inputs.directory}))?;
    let files = document::array(&files, "生产者启动前文件")?
        .iter()
        .filter(|row| {
            row["path"]
                .as_str()
                .is_none_or(|path| !FILES.contains(&path))
        })
        .cloned()
        .collect::<Vec<_>>();
    inventory::request(
        &json!({"backend":inputs.backend,"directory":inputs.directory,"inputs":files}),
        true,
    )?;
    let intent = json!({"format_version":2,"kind":"restore-source-producer-intent","operation_id":operation,
        "controller":controller,"source_generation":inputs.start,"dataset_lineage":inputs.lineage,
        "executable":inputs.tool,"source":common::binding(&inputs.backend.join("xtask/src/recovery/dataset_prepare/existing_native.rs"))?,
        "configuration":common::binding(&configuration_path)?,"argv":command["argv"],"cwd":inputs.backend,
        "environment_sha256":digest::plan_hash(&json!(inputs.environment))?,"started_at":chrono::Utc::now().to_rfc3339(),
        "coordinator_source":inputs.coordinator,"execution_sha":inputs.execution_sha,"staging":descriptor,"inputs":files});
    document::write_new(
        &inputs.directory.join("source-producer-intent.json"),
        &intent,
    )?;
    inputs.source_unchanged()?;
    inputs.staging(&intent)?;
    Ok(intent)
}

pub(super) fn register(inputs: &Inputs<'_>, intent: &Value, pid: u32) -> Result<Value> {
    let identity = common::process_identity(pid)?.ok_or("来源生产者在登记内核身份前退出")?;
    let executable = Path::new(document::text(&identity["executable"], "生产者可执行文件")?);
    if executable.canonicalize()? != inputs.tool.canonicalize()? {
        return Err("来源生产者实际可执行文件与绑定工具不同".into());
    }
    let producer = json!({"format_version":1,"kind":"restore-source-producer","operation_id":intent["operation_id"],
        "intent":common::binding(&inputs.directory.join("source-producer-intent.json"))?,"controller":intent["controller"],
        "process":identity,"executable":inputs.tool,"argv":intent["argv"]});
    document::write_new(&inputs.directory.join("source-producer.json"), &producer)?;
    unchanged(&producer)?;
    Ok(producer)
}

pub(super) fn unchanged(producer: &Value) -> Result<()> {
    let pid = producer["process"]["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or("生产者 PID 无效")?;
    if common::process_identity(std::process::id())?.as_ref() != Some(&producer["controller"])
        || common::process_identity(pid)?.as_ref() != Some(&producer["process"])
    {
        return Err("来源验收在启动授权前控制器或生产者内核身份变化".into());
    }
    Ok(())
}

pub(in crate::recovery::source::evidence) fn write_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<Value> {
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    common::binding(path)
}

pub(super) fn complete(
    inputs: &Inputs<'_>,
    producer: &Value,
    ready: &Value,
    stdout: &[u8],
    stderr: &[u8],
    code: i32,
) -> Result<()> {
    let stdout = write_bytes(
        &inputs.directory.join("source-producer.stdout.json"),
        stdout,
    )?;
    let stderr = write_bytes(&inputs.directory.join("source-producer.stderr.log"), stderr)?;
    document::write_new(
        &inputs.directory.join("source-producer-completion.json"),
        &json!({"format_version":1,
        "kind":"restore-source-producer-completion","operation_id":producer["operation_id"],
        "intent":common::binding(&inputs.directory.join("source-producer-intent.json"))?,
        "producer":common::binding(&inputs.directory.join("source-producer.json"))?,"ready":ready,"stdout":stdout,
        "stderr":stderr,"process":producer["process"],"exit_code":code,"completed_at":chrono::Utc::now().to_rfc3339()}),
    )
}

//! 来源生产者停止的完整只读核验；不猜测未知启动、不发送信号、不补写任何收据。
use super::{environment, execution::saved, inventory, producer, ready, running};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["backend", "start"], "来源生产者停止核验参数")?;
    let backend = super::binding::path(&args["backend"])?;
    require(&backend, &args["start"])
}

pub(super) fn require(backend: &Path, start: &Value) -> Result<Value> {
    let facts = running::load(backend, start, false, true)?;
    let directory =
        PathBuf::from(document::text(&facts["output"], "来源运行代次")?).join("verification");
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"status":"not_started","process":null}));
        }
        Err(error) => {
            return Err(format!(
                "无法读取来源 verification：{error}；核对同代路径和权限，禁止猜测或清理进程"
            )
            .into());
        }
        Ok(metadata) if digest::linked(&metadata) || !metadata.is_dir() => {
            return Err("来源验收 verification 必须为普通目录，不能是链接或重解析点".into());
        }
        Ok(_) => {}
    }
    let names = entries(backend, &directory)?;
    let private: BTreeMap<String, String> =
        serde_json::from_value(facts["environment"]["environment"].clone())?;
    let environment = environment::configured(&private)?;
    let intent = document::read(&directory.join("source-producer-intent.json"))?;
    let tool = crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path(
        Path::new(document::text(&intent["executable"], "来源生产者原生工具")?))?;
    let inputs = producer::Inputs {
        backend,
        execution: Path::new(document::text(&facts["execution"], "来源执行根")?),
        directory: &directory,
        start,
        lineage: &facts["receipt"]["dataset_lineage"],
        environment: &environment,
        tool: &tool,
        coordinator: &facts["coordinator_source"],
        execution_sha: document::text(&facts["request"]["expected_backend_sha"], "来源执行 SHA")?,
        historical: true,
    };
    let registered = inputs.registered()?;
    if names.contains("source-producer-ready.json") {
        inputs.ready(&registered["intent"])?;
    }
    inputs.stopped(&registered["producer"])?;
    let value = completion(backend, &inputs, &registered, &names)?;
    if entries(backend, &directory)? != names
        || inputs.registered()? != registered
        || running::load(backend, start, false, true)? != facts
    {
        return Err("来源生产者停止核验期间完整历史、登记或证据集合变化".into());
    }
    inputs.stopped(&registered["producer"])?;
    Ok(value)
}

fn entries(backend: &Path, directory: &Path) -> Result<BTreeSet<String>> {
    common::local_directory(backend, directory, "来源验收 verification")?;
    let mut allowed = producer::launch::FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    allowed.extend(
        [
            "before",
            "after",
            "audit",
            "source-tools",
            "cache-cleanup.json",
            "source-runtime.json",
            "failed.json",
            "source-verification-recovery-intent.json",
            "source-verification-recovery.json",
            "recovery-audit",
        ]
        .map(str::to_owned),
    );
    let mut names = BTreeSet::new();
    for entry in crate::recovery::runtime::dataset_authority::reads::entries(directory)? {
        let entry = entry?;
        if digest::linked(&std::fs::symlink_metadata(entry.path())?) {
            return Err("来源验收包含链接或重解析点".into());
        }
        names.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| "来源验收文件名必须为 UTF-8")?,
        );
    }
    if names.contains("cache-cleanup-attempt.json")
        || names.contains("cache-cleanup-execution.json")
    {
        let state =
            ready::policy::cache_attempt::state(&json!({"backend":backend,"directory":directory,
            "output":directory.join("cache-cleanup.json")}))?;
        for (field, name) in [
            ("attempt", "cache-cleanup-attempt.json"),
            ("execution", "cache-cleanup-execution.json"),
        ] {
            if !state[field].is_null() {
                allowed.insert(name.into());
            }
        }
    }
    if !names.is_subset(&allowed)
        || !names.contains("source-producer-intent.json")
        || !names.contains("source-producer.json")
    {
        return Err("来源验收缺少已登记进程身份或包含未知证据".into());
    }
    Ok(names)
}

fn completion(
    backend: &Path,
    inputs: &producer::Inputs<'_>,
    registered: &Value,
    names: &BTreeSet<String>,
) -> Result<Value> {
    let identity = &registered["producer"]["process"];
    let directory = inputs.directory;
    if names.contains("source-runtime.json") {
        let verified = saved::verify(
            backend,
            &common::binding(&directory.join("source-runtime.json"))?,
            false,
        )?;
        if verified["receipt"]["source_generation"] != *inputs.start {
            return Err("来源验收完成收据不属于当前 source generation".into());
        }
        return Ok(json!({"status":"verified_stopped","process":identity}));
    }
    let stdout = names.contains("source-producer.stdout.json");
    let stderr = names.contains("source-producer.stderr.log");
    let completed = names.contains("source-producer-completion.json");
    if (stdout || stderr || completed) && !names.contains("source-producer-ready.json")
        || stderr && !stdout
        || completed && !(stdout && stderr)
    {
        return Err("来源验收生产者输出发布顺序不成立".into());
    }
    let completion = if completed {
        inputs.completion(registered)?
    } else {
        Value::Null
    };
    if names.contains("failed.json") {
        let failure = common::read_bound(
            backend,
            &common::binding(&directory.join("failed.json"))?,
            "来源验收失败记录",
        )?;
        document::exact(&failure, &["status", "error"], "来源验收失败记录")?;
        if failure["status"] != "failed" {
            return Err("来源验收失败记录状态无效".into());
        }
        document::text(&failure["error"], "来源验收失败类型")?;
    }
    let evidence = inventory::request(
        &json!({"backend":backend,"directory":directory,
        "inputs":registered["intent"]["inputs"]}),
        true,
    )?;
    let directories = directories(directory, directory)?;
    inputs.stopped(&registered["producer"])?;
    Ok(
        json!({"status":"stopped","process":identity,"evidence":{"files":evidence,"directories":directories},
        "exit_code":completion["exit_code"]}),
    )
}

fn directories(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in crate::recovery::runtime::dataset_authority::reads::entries(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if digest::linked(&metadata) {
            return Err("来源验收证据子树包含链接或重解析点".into());
        }
        if metadata.is_dir() {
            names.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .ok_or("来源证据目录名必须为 UTF-8")?
                    .replace('\\', "/"),
            );
            names.extend(directories(root, &entry.path())?);
        } else if !metadata.is_file() {
            return Err("来源验收证据子树包含非普通文件".into());
        }
    }
    names.sort();
    Ok(names)
}

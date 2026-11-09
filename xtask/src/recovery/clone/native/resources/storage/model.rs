use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use super::super::{
    super::{digest, json as json_io, lifecycle::manifest},
    common, origin,
};
use crate::{Result, cli::CloneSide, local_test_path::validate_external_existing_file};

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "side",
    "manifest",
    "original",
    "environment",
    "scope_id",
    "executable",
    "data_directory",
    "api_url",
    "console_url",
    "credential_files",
    "previous",
    "timeout_seconds",
];

pub(crate) struct Validated {
    pub(crate) private: BTreeMap<String, String>,
}

pub(crate) fn validate(
    backend: &Path,
    run_dir: &Path,
    manifest: &Value,
    side: CloneSide,
    request: &Value,
) -> Result<Validated> {
    let object = json_io::exact(request, FIELDS, "存储重启请求")?;
    if object.get("format_version").and_then(Value::as_u64) != Some(1)
        || object.get("kind").and_then(Value::as_str) != Some("devex-clone-storage-restart")
        || object.get("side").and_then(Value::as_str) != Some(side.as_str())
        || object.get("manifest") != Some(&common::binding(&run_dir.join("manifest.json"))?)
        || object.get("original")
            != Some(
                manifest
                    .get(if side == CloneSide::Source {
                        "source_request"
                    } else {
                        "initialized"
                    })
                    .ok_or("复制清单缺少存储原始侧输入")?,
            )
        || object.get("environment")
            != Some(
                manifest
                    .get(format!("{}_environment", side.as_str()))
                    .ok_or("复制清单缺少存储环境")?,
            )
    {
        return Err("存储重启必须属于同一复制运行和原始侧输入".into());
    }
    let scope = object
        .get("scope_id")
        .and_then(Value::as_str)
        .ok_or("存储重启缺少 scope_id")?;
    common::scope_id(scope)?;
    common::directory_identity(backend, object.get("data_directory").unwrap())?;
    let executable = manifest::validate_binding_shape(&object["executable"], "RustFS 可执行文件")?;
    let path = Path::new(json_io::text(&executable["path"], "RustFS 可执行文件")?);
    let path = validate_external_existing_file(path)
        .map_err(|error| format!("RustFS 可执行文件无效：{error}"))?;
    let actual = digest::file(&path)?;
    if executable["bytes"].as_u64() != Some(actual.bytes)
        || executable["sha256"].as_str() != Some(actual.sha256.as_str())
    {
        return Err("RustFS 可执行文件与登记大小或摘要不一致".into());
    }
    let previous = json_io::exact(
        object.get("previous").unwrap(),
        &[
            "identity",
            "process_receipt",
            "launch_receipt",
            "ready_receipt",
        ],
        "RustFS 原启动代次",
    )?;
    let previous_identity = common::validate_process_identity(previous.get("identity").unwrap())?;
    if Path::new(previous_identity["executable"].as_str().unwrap()) != path {
        return Err("存储重启不能替换原可执行文件".into());
    }
    let api = object
        .get("api_url")
        .and_then(Value::as_str)
        .ok_or("存储重启缺少 API 地址")?;
    let console = object
        .get("console_url")
        .and_then(Value::as_str)
        .ok_or("存储重启缺少控制台地址")?;
    common::endpoint(api)?;
    common::endpoint(console)?;
    if api == console {
        return Err("RustFS 两个监听端点必须不同".into());
    }
    let timeout = object
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .filter(|value| (1..=180).contains(value))
        .ok_or("RustFS 就绪期限必须为 1 至 180 秒")?;
    let credentials = json_io::exact(
        object.get("credential_files").unwrap(),
        &["access_key", "secret_key"],
        "RustFS 凭据",
    )?;
    for descriptor in credentials.values() {
        common::bound_file(backend, descriptor, "RustFS 凭据文件")?;
    }
    let private = common::environment(backend, object.get("environment").unwrap())?;
    let selected = original_inputs(backend, manifest, request, side)?;
    for key in ["access_key", "secret_key"] {
        let descriptor = credentials.get(key).unwrap();
        let path = common::bound_file(backend, descriptor, "RustFS 凭据文件")?;
        let contents = fs::read_to_string(path)?.trim().to_owned();
        let configured = selected
            .pointer(&format!("/s3/{key}"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("原业务对象存储缺少明文凭据")?;
        if contents.is_empty() || configured != contents {
            return Err("存储凭据文件与原业务认证不符".into());
        }
    }
    let _ = timeout;
    Ok(Validated { private })
}

pub(crate) fn arguments(request: &Value) -> Result<Vec<String>> {
    let executable = request
        .pointer("/executable/path")
        .and_then(Value::as_str)
        .ok_or("RustFS 请求缺少可执行文件")?;
    let api = request
        .get("api_url")
        .and_then(Value::as_str)
        .ok_or("RustFS 请求缺少 API 地址")?;
    let console = request
        .get("console_url")
        .and_then(Value::as_str)
        .ok_or("RustFS 请求缺少控制台地址")?;
    let (_, api) = common::endpoint(api)?;
    let (_, console) = common::endpoint(console)?;
    Ok(vec![
        executable.to_owned(),
        "server".into(),
        "--address".into(),
        api,
        "--console-address".into(),
        console,
        "--console-enable".into(),
        "--access-key-file".into(),
        json_io::text(
            &request["credential_files"]["access_key"]["path"],
            "RustFS access key 文件路径",
        )?
        .into(),
        "--secret-key-file".into(),
        json_io::text(
            &request["credential_files"]["secret_key"]["path"],
            "RustFS secret key 文件路径",
        )?
        .into(),
        request
            .pointer("/data_directory/path")
            .and_then(Value::as_str)
            .ok_or("RustFS 请求缺少数据目录")?
            .to_owned(),
    ])
}

pub(crate) fn clear_overrides(command: &mut std::process::Command) {
    let keys = command
        .get_envs()
        .filter_map(|(key, _)| {
            let name = key.to_str()?.to_ascii_uppercase();
            (name.starts_with("RUSTFS_") || name.starts_with("MINIO_")).then(|| key.to_owned())
        })
        .collect::<Vec<_>>();
    for key in keys {
        command.env_remove(key);
    }
}

fn original_inputs(
    backend: &Path,
    manifest: &Value,
    request: &Value,
    side: CloneSide,
) -> Result<Value> {
    let original = common::read_bound(
        backend,
        request.get("original").ok_or("存储请求缺少原始输入")?,
        "存储原始输入",
    )?;
    let previous = request
        .get("previous")
        .and_then(Value::as_object)
        .ok_or("存储请求缺少原代次")?;
    let process = common::read_bound(
        backend,
        previous.get("process_receipt").unwrap(),
        "RustFS 原进程收据",
    )?;
    let launch = common::read_bound(
        backend,
        previous.get("launch_receipt").unwrap(),
        "RustFS 原启动收据",
    )?;
    let ready = common::read_bound(
        backend,
        previous.get("ready_receipt").unwrap(),
        "RustFS 原就绪收据",
    )?;
    let identity = common::validate_process_identity(previous.get("identity").unwrap())?;
    let scope = request
        .get("scope_id")
        .and_then(Value::as_str)
        .ok_or("存储请求缺少 scope")?;
    let data_dir = request
        .pointer("/data_directory/path")
        .and_then(Value::as_str)
        .ok_or("存储请求缺少数据目录")?;
    for key in ["identity", "scope_id", "data_dir"] {
        let expected = if key == "identity" {
            &identity
        } else if key == "scope_id" {
            &Value::String(scope.into())
        } else {
            &Value::String(data_dir.into())
        };
        if process.get(key) != Some(expected) {
            return Err("原存储进程不属于明确 scope、数据目录及创建代次".into());
        }
    }
    let selected = match side {
        CloneSide::Source => source_inputs(
            backend, request, &original, &process, &launch, &ready, &identity,
        )?,
        CloneSide::Target => target_inputs(
            backend, manifest, request, &process, &launch, &ready, &identity,
        )?,
    };
    if selected.pointer("/s3/endpoint").and_then(Value::as_str)
        != request.get("api_url").and_then(Value::as_str)
    {
        return Err("存储端口不是原业务侧对象端点".into());
    }
    Ok(selected)
}

fn source_inputs(
    backend: &Path,
    request: &Value,
    original: &Value,
    process: &Value,
    launch: &Value,
    ready: &Value,
    identity: &Value,
) -> Result<Value> {
    let selected = original.get("source").ok_or("来源请求缺少 source")?.clone();
    if ready.get("status").and_then(Value::as_str) != Some("ready")
        || ready.get("identity") != Some(identity)
        || ready.get("process_receipt") != request.pointer("/previous/process_receipt")
        || ready.get("data_dir").and_then(Value::as_str)
            != request
                .pointer("/data_directory/path")
                .and_then(Value::as_str)
        || ready.get("listeners")
            != Some(&Value::Array(vec![
                request["api_url"].clone(),
                request["console_url"].clone(),
            ]))
        || process.get("launch_plan") != request.pointer("/previous/launch_receipt")
        || process.get("command")
            != Some(&Value::Array(
                arguments(request)?.into_iter().map(Value::String).collect(),
            ))
        || launch.get("executable") != request.get("executable")
        || launch.get("data_dir").and_then(Value::as_str)
            != request
                .pointer("/data_directory/path")
                .and_then(Value::as_str)
        || launch.get("scope_id") != request.get("scope_id")
    {
        return Err("来源服务缺少同一原始启动计划和已完成就绪证据".into());
    }
    for key in ["api_url", "console_url"] {
        if launch.get(key) != request.get(key) {
            return Err("来源存储端口不属于原启动计划".into());
        }
    }
    for key in ["access_key", "secret_key"] {
        let expected = request.pointer(&format!("/credential_files/{key}"));
        if launch.get(format!("{key}_file")).and_then(Value::as_str)
            != expected
                .and_then(|value| value.get("path"))
                .and_then(Value::as_str)
            || process
                .get("secret_files")
                .and_then(|value| value.get(format!("{key}_file")))
                != expected
        {
            return Err("来源凭据不是原实际启动文件".into());
        }
    }
    let _ = backend;
    Ok(selected)
}

fn target_inputs(
    backend: &Path,
    manifest: &Value,
    request: &Value,
    process: &Value,
    launch: &Value,
    ready: &Value,
    identity: &Value,
) -> Result<Value> {
    json_io::exact(
        launch,
        &[
            "format_version",
            "scope_id",
            "identity",
            "arguments",
            "credential_files",
        ],
        "RustFS launch",
    )?;
    let initialized = origin::initialized(
        backend,
        manifest
            .get("initialized")
            .ok_or("复制清单缺少目标初始化收据")?,
    )?;
    if initialized.initial.get("status").and_then(Value::as_str) != Some("fresh_target_initialized")
        || initialized
            .initial
            .get("prepare_sha256")
            .and_then(Value::as_str)
            != Some(
                super::super::super::digest::file(&initialized.root.join("prepare.json"))?
                    .sha256
                    .as_str(),
            )
    {
        return Err("目标原始初始化发布与准备记录不同".into());
    }
    let selected = initialized
        .request
        .get("target")
        .cloned()
        .ok_or("目标原始请求缺少 target")?;
    let old = initialized
        .request
        .pointer("/storage/rustfs")
        .ok_or("目标原始请求缺少 RustFS 代次")?;
    let expected = initialized
        .review
        .pointer("/services/rustfs")
        .ok_or("目标审阅计划缺少 RustFS")?;
    let expected_old = json!({
        "identity": identity,
        "sha256": request.pointer("/executable/sha256"),
        "process_receipt": request.pointer("/previous/process_receipt"),
        "launch_receipt": request.pointer("/previous/launch_receipt"),
    });
    if old != &expected_old
        || request.pointer("/previous/ready_receipt")
            != request.pointer("/previous/process_receipt")
        || process.get("lifecycle").and_then(Value::as_str) != Some("running")
        || expected.get("scope_id") != request.get("scope_id")
        || expected.get("data_dir").and_then(Value::as_str)
            != request
                .pointer("/data_directory/path")
                .and_then(Value::as_str)
        || expected.get("api") != request.get("api_url")
        || expected.get("console") != request.get("console_url")
        || launch.get("identity") != Some(identity)
        || launch.get("arguments")
            != Some(&Value::Array(
                arguments(request)?.into_iter().map(Value::String).collect(),
            ))
        || launch.get("credential_files") != request.get("credential_files")
    {
        return Err("目标重启不是原初始化中完整相同存储的代次过渡".into());
    }
    if ready.get("identity") != Some(identity) {
        return Err("目标原就绪收据不属于原存储进程".into());
    }
    Ok(selected)
}

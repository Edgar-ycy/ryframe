use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{
    digest, json as json_io,
    lifecycle::{
        manifest,
        stage::{self, StageRequest},
    },
};
use super::{input::TargetContext, tree};
use crate::Result;

/// 历史谱系暂由已有只读核验器提供，原生代码仍复核字段、实际摘要和完整目录树。
pub(crate) trait Observer {
    fn inspect(&self, backend: &Path, stage: &StageRequest<'_>) -> Result<Value>;
}

pub(crate) struct SystemObserver;

impl Observer for SystemObserver {
    fn inspect(&self, backend: &Path, request: &StageRequest<'_>) -> Result<Value> {
        let readonly = StageRequest {
            attempt: None,
            ..*request
        };
        stage::invoke(backend, &readonly, "target-authority")
    }
}

pub(crate) fn inspect(
    backend: &Path,
    request: &StageRequest<'_>,
    observer: &impl Observer,
) -> Result<Value> {
    let value = observer.inspect(backend, request)?;
    validate(backend, request.run_dir, &value)?;
    Ok(value)
}

pub(crate) fn validate(backend: &Path, directory: &Path, value: &Value) -> Result<()> {
    json_io::exact(
        value,
        &[
            "format_version",
            "kind",
            "initialized",
            "request",
            "initialized_files",
            "files",
            "generation",
            "remote_writes",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ],
        "目标只读权威",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-target-authority"
        || value["remote_writes"].as_u64() != Some(0)
        || ["target_ready", "clone_verified", "restore_qualified"]
            .iter()
            .any(|key| value[*key].as_bool() != Some(false))
    {
        return Err("目标只读权威类型、写入或资格声明无效".into());
    }
    let manifest_value = manifest::load(backend, &directory.join("manifest.json"))?.value;
    if manifest_value["copy_stage"] != "source_to_seed"
        || value["initialized"] != manifest_value["initialized"]
    {
        return Err("目标只读权威不属于当前固定运行清单".into());
    }
    let path = manifest::bound_file(backend, &value["initialized"], "目标权威 initialized", true)?;
    let target = path.parent().ok_or("目标初始化收据缺少父目录")?;
    require_unlocked(target)?;
    if path.file_name().and_then(|name| name.to_str()) != Some("initialized.json") {
        return Err("目标权威必须绑定本代次 initialized.json".into());
    }
    let request_path = manifest::bound_file(backend, &value["request"], "目标权威 request", true)?;
    let prepare = json_io::read(&target.join("prepare.json"))?;
    if prepare["request"] != value["request"]
        || json_io::read(&request_path)? != json_io::read(&target.join("request.json"))?
    {
        return Err("目标权威请求未绑定同一初始化前像".into());
    }
    let initialized_files = manifest::bound_file(
        backend,
        &value["initialized_files"],
        "目标权威文件谱系",
        true,
    )?;
    if Some(initialized_files.as_path())
        != target
            .parent()
            .map(|parent| parent.join("initialized-files.json"))
            .as_deref()
        || tree::snapshot(backend, target, false)? != value["files"]
    {
        return Err("目标权威完整文件树或谱系路径不同".into());
    }
    let guard = json!({"path": "copy-target.guard", "type": "file", "bytes": 1,
        "sha256": hex::encode(Sha256::digest([0]))});
    if !json_io::array(&value["files"], "目标完整文件树")?.contains(&guard) {
        return Err("目标初始化缺少已登记的规范控制互斥文件".into());
    }
    let mut generation = json_io::read(&path)?["generation"].clone();
    generation
        .as_object_mut()
        .ok_or("目标初始化缺少 generation 对象")?
        .remove("storage");
    if generation != value["generation"] {
        return Err("目标权威当前代次不属于初始化证据".into());
    }
    Ok(())
}

fn require_unlocked(target: &Path) -> Result<()> {
    match fs::symlink_metadata(target.join("initialize.lock")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法核对目标初始化锁：{error}").into()),
        Ok(_) => Err("目标初始化锁仍存在，拒绝观察其他调用方持有的代次".into()),
    }
}

pub(crate) fn protected_paths(context: &TargetContext, authority: &Value) -> Result<Vec<PathBuf>> {
    let mut paths = tree::files(&context.target_dir, &authority["files"])?;
    paths.push(context.run_dir.join("manifest.json"));
    paths.push(context.request_path.clone());
    paths.extend(auxiliary_inputs(context)?);
    paths.extend(tool_inputs(context)?);
    paths.extend(configuration_inputs(authority)?);
    paths.extend(storage_inputs(context)?);
    for field in ["source_environment", "target_environment"] {
        paths.push(manifest::bound_file(
            &context.backend,
            &context.manifest[field],
            field,
            true,
        )?);
    }
    for descriptor in context.manifest["build_bridges"]
        .as_array()
        .ok_or("构建桥接必须为列表")?
    {
        let path = PathBuf::from(json_io::text(&descriptor["path"], "构建桥接路径")?);
        if digest::file(&path)?.sha256 != json_io::text(&descriptor["sha256"], "构建桥接摘要")?
        {
            return Err("构建桥接文件变化".into());
        }
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn auxiliary_inputs(context: &TargetContext) -> Result<Vec<PathBuf>> {
    let workspace = context.target_dir.parent().ok_or("目标证据缺少工作区")?;
    let mut paths = Vec::new();
    for name in [
        "initialized-files.json",
        "prepared-files.json",
        "registration.json",
    ] {
        let path = workspace.join(name);
        let document = json_io::read(&path)?;
        for field in ["predecessor", "registration"] {
            if let Some(binding) = document.get(field) {
                if field == "predecessor" && binding.is_null() {
                    continue;
                }
                paths.push(manifest::bound_file(
                    &context.backend,
                    binding,
                    "目标辅助谱系",
                    true,
                )?);
            }
        }
        paths.push(path);
    }
    if let Some(execution) = context.request.get("execution_backend") {
        paths.push(manifest::bound_file(
            &context.backend,
            &execution["fixture"],
            "冻结业务 fixture",
            true,
        )?);
    }
    Ok(paths)
}

fn tool_inputs(context: &TargetContext) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut binding = json_io::object(&context.request["review"], "目标审阅绑定")?.clone();
    binding.remove("canonical_sha256");
    paths.push(manifest::bound_file(
        &context.backend,
        &Value::Object(binding),
        "目标审阅",
        true,
    )?);
    let build = manifest::bound_file(
        &context.backend,
        &context.request["maintenance_build"],
        "目标维护构建",
        true,
    )?;
    let receipt = json_io::read(&build)?;
    for artifact in json_io::object(&receipt["artifacts"], "目标维护产物")?.values() {
        paths.push(PathBuf::from(json_io::text(
            &artifact["executable"],
            "目标维护产物路径",
        )?));
    }
    paths.push(build);
    for database in context.databases()? {
        paths.push(PathBuf::from(json_io::text(
            &database["defaults_file"],
            "目标 MySQL 凭据路径",
        )?));
    }
    paths.push(context.tool("mysql")?);
    Ok(paths)
}

fn configuration_inputs(authority: &Value) -> Result<Vec<PathBuf>> {
    let config = &authority["generation"]["configuration"];
    let directory = Path::new(json_io::text(&config["directory"], "目标配置目录")?);
    json_io::object(&config["files"], "目标配置文件摘要")?
        .keys()
        .map(|name| {
            if Path::new(name).file_name().and_then(|part| part.to_str()) != Some(name)
                || name.contains(['/', '\\', ':'])
            {
                return Err("目标配置文件路径越界".into());
            }
            Ok(directory.join(name))
        })
        .collect()
}

fn storage_inputs(context: &TargetContext) -> Result<Vec<PathBuf>> {
    let storage = context.storage();
    let mut paths = vec![
        PathBuf::from(json_io::text(
            &storage["rustfs"]["identity"]["executable"],
            "RustFS 执行路径",
        )?),
        PathBuf::from(json_io::text(
            &storage["redis"]["wsl"]["path"],
            "Redis WSL 执行路径",
        )?),
    ];
    for descriptor in [
        &storage["rustfs"]["process_receipt"],
        &storage["rustfs"]["launch_receipt"],
        &storage["redis"]["configuration"],
    ] {
        paths.push(manifest::bound_file(
            &context.backend,
            descriptor,
            "目标存储输入",
            true,
        )?);
    }
    let launch_path = manifest::bound_file(
        &context.backend,
        &storage["rustfs"]["launch_receipt"],
        "RustFS launch",
        true,
    )?;
    let launch = json_io::read(&launch_path)?;
    let credentials = json_io::exact(
        &launch["credential_files"],
        &["access_key", "secret_key"],
        "RustFS 凭据文件",
    )?;
    for descriptor in credentials.values() {
        paths.push(manifest::bound_file(
            &context.backend,
            descriptor,
            "RustFS 凭据文件",
            true,
        )?);
    }
    Ok(paths)
}

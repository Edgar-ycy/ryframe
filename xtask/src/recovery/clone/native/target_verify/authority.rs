use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{
    bridge::scope::Scope,
    cache::windows_tool,
    digest, json as json_io,
    lifecycle::{manifest, stage::StageRequest},
    resources::common,
};
use super::{
    guard,
    input::{TargetContext, request},
    published, tree,
};
use crate::Result;

#[path = "authority/generation.rs"]
pub(crate) mod generation;
#[path = "authority/ports.rs"]
mod ports;
pub(crate) use ports::require_closed;

/// 实际历史和代次观察均在本进程执行；测试替身不能成为执行许可。
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
        observe_with(backend, &readonly, |backend, target, environment, scope| {
            generation::observe(
                backend,
                target,
                environment,
                &mut |root, path, receipt| scope.maintenance(root, path, receipt),
                &mut ports::require_closed,
            )
        })
    }
}

/// 前后完整复核与真实代次观察共用同一来源窗口；不创建锁或观察目录。
pub(crate) fn observe_with(
    backend: &Path,
    request: &StageRequest<'_>,
    observe: impl FnOnce(
        &Path,
        &Value,
        &std::collections::BTreeMap<String, String>,
        &Scope,
    ) -> Result<Value>,
) -> Result<Value> {
    if request.stage != "target-verify" || request.mode != "run" || request.attempt.is_some() {
        return Err("目标只读权威只接受无attempt的target-verify/run观察".into());
    }
    let original = manifest::load(backend, &request.run_dir.join("manifest.json"))?;
    let filename =
        manifest::bound_file(backend, &original.value["initialized"], "目标初始化", true)?;
    let (registered_path, registration) = published::tree::registered(backend, &filename)?;
    let target = registered_path.parent().ok_or("目标初始化缺少目录")?;
    let scope = Scope::load(
        backend,
        json_io::array(&original.value["build_bridges"], "构建桥接")?,
        &[],
    )?;
    let (initialized, target_request) = published::unlocked_history(backend, &filename)?;
    let prepare = json_io::read(&target.join("prepare.json"))?;
    let files = tree::snapshot(backend, target, false)?;
    require_arm_target(
        backend,
        &original.value,
        target,
        &prepare["request"],
        &target_request,
        &files,
    )?;
    let inputs = guard::Inputs::open(tree::files(target, &files)?)?;
    let environment = common::environment(backend, &original.value["target_environment"])?;
    let generation = observe(backend, &target_request, &environment, &scope)?;
    let mut expected = json_io::object(&initialized["generation"], "初始化generation")?.clone();
    expected.remove("storage");
    if generation != Value::Object(expected) {
        return Err("目标当前源码、维护工具、配置或物理连接不同于初始化代次".into());
    }
    let value = json!({
        "format_version":1,"kind":"devex-clone-target-authority",
        "initialized":original.value["initialized"],"request":prepare["request"],
        "initialized_files":common::binding(&target.parent().ok_or("目标缺少工作区")?.join("initialized-files.json"))?,
        "files":files,"generation":generation,"remote_writes":0,
        "target_ready":false,"clone_verified":false,"restore_qualified":false
    });
    inputs.check()?;
    let current = manifest::load(backend, &request.run_dir.join("manifest.json"))?;
    if published::unlocked_history(backend, &filename)? != (initialized, target_request)
        || tree::snapshot(backend, target, false)? != files
        || current.digest != original.digest
        || current.value != original.value
    {
        return Err("目标只读权威核验期间输入或完整历史变化".into());
    }
    scope.assert_unchanged()?;
    registration.check()?;
    validate(backend, request.run_dir, &value)?;
    Ok(value)
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
    if value["initialized"] != manifest_value["initialized"] {
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
    let target_request = json_io::read(&request_path)?;
    if prepare["request"] != value["request"]
        || target_request != json_io::read(&target.join("request.json"))?
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
    if manifest_value["copy_stage"] == "seed_to_arm"
        && value["initialized_files"] != manifest_value["target_initialized_files"]
    {
        return Err("seed_to_arm 权威文件谱系不同于固定运行清单".into());
    }
    require_arm_target(
        backend,
        &manifest_value,
        target,
        &value["request"],
        &target_request,
        &value["files"],
    )?;
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

fn require_arm_target(
    backend: &Path,
    manifest: &Value,
    target: &Path,
    request: &Value,
    target_request: &Value,
    current_files: &Value,
) -> Result<()> {
    if manifest["copy_stage"] != "seed_to_arm" {
        return Ok(());
    }
    if !matches!(target_request["side"].as_str(), Some("base" | "candidate")) {
        return Err("seed_to_arm 目标权威只接受实际 base 或 candidate 请求".into());
    }
    let workspace = target.parent().ok_or("arm 目标缺少初始化工作区")?;
    let registration_path = workspace.join("registration.json");
    let files_path = workspace.join("initialized-files.json");
    let prepared_path = workspace.join("prepared-files.json");
    let guard = guard::Inputs::open([
        registration_path.clone(),
        files_path.clone(),
        prepared_path.clone(),
    ])?;
    if common::binding(&registration_path)? != manifest["target_registration"]
        || common::binding(&files_path)? != manifest["target_initialized_files"]
    {
        return Err("seed_to_arm 清单未绑定初始化工作区的原登记和文件谱系".into());
    }
    let registration =
        common::read_bound(backend, &manifest["target_registration"], "arm 原目标登记")?;
    let files = common::read_bound(
        backend,
        &manifest["target_initialized_files"],
        "arm 原目标文件谱系",
    )?;
    json_io::exact(
        &registration,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "arm 原目标登记",
    )?;
    json_io::exact(
        &files,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "arm 原目标文件谱系",
    )?;
    if registration["format_version"] != 1
        || registration["kind"] != "devex-clone-fresh-target-registration"
        || registration["request"] != *request
        || registration["environment"] != manifest["target_environment"]
        || Path::new(json_io::text(
            &registration["target_directory"],
            "arm 原目标目录",
        )?) != target
        || files["format_version"] != 1
        || files["kind"] != "devex-clone-fresh-target-initialized-files"
        || files["registration"] != manifest["target_registration"]
        || files["predecessor"] != common::binding(&prepared_path)?
        || files["files"] != *current_files
    {
        return Err("seed_to_arm 目标原登记、环境、请求或文件谱系不同".into());
    }
    guard.check()
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
        if execution.get("execution_source").is_some() {
            paths.extend(
                crate::recovery::fixture_control::source_pair::execution::target_protected_inputs(
                    &context.backend,
                    execution,
                )?,
            );
            return Ok(paths);
        }
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
    let mut paths = vec![PathBuf::from(json_io::text(
        &storage["rustfs"]["identity"]["executable"],
        "RustFS 执行路径",
    )?)];
    let redis = &storage["redis"];
    if redis["platform"] == "windows" {
        for descriptor in windows_tool::package_bindings(&redis["tool"])? {
            paths.push(request::external_file(&descriptor)?);
        }
    } else {
        paths.push(PathBuf::from(json_io::text(
            &redis["wsl"]["path"],
            "Redis WSL 执行路径",
        )?));
    }
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

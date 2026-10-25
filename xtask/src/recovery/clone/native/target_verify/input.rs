use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::super::{
    digest,
    json::{self as json_io, exact},
    lifecycle::{manifest, stage::StageRequest},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

#[cfg(test)]
#[path = "../../../../../tests/internal/target_configuration_toml.rs"]
pub(crate) mod configuration_fixture;
#[path = "request.rs"]
pub(crate) mod request;
#[path = "request_configuration.rs"]
pub(crate) mod request_configuration;
#[path = "request_evidence.rs"]
pub(crate) mod request_evidence;
#[path = "request_physical.rs"]
pub(crate) mod request_physical;
#[path = "request_review.rs"]
pub(crate) mod request_review;
#[cfg(test)]
#[path = "../../../../../tests/internal/target_request_native.rs"]
mod request_tests;
pub(crate) struct TargetContext {
    pub(crate) backend: PathBuf,
    pub(crate) run_dir: PathBuf,
    pub(crate) manifest: Value,
    pub(crate) target_dir: PathBuf,
    pub(crate) initialized_path: PathBuf,
    pub(crate) initialized_binding: Value,
    pub(crate) initialized: Value,
    pub(crate) request_path: PathBuf,
    pub(crate) request: Value,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) observation_dir: PathBuf,
}

impl TargetContext {
    pub(crate) fn storage_inputs(&self) -> super::storage::StorageInputs<'_> {
        super::storage::StorageInputs {
            backend: &self.backend,
            request: &self.request,
            environment: &self.environment,
            observation_dir: &self.observation_dir,
        }
    }

    pub(crate) fn load(backend: &Path, request: &StageRequest<'_>) -> Result<Self> {
        let attempt = request.attempt.ok_or("target-verify 执行缺少阶段序号")?;
        let inputs = read_inputs(backend, request)?;
        let observation_dir = request.run_dir.join(format!("v{attempt:04}"));
        validate_local_test_path(&observation_dir, backend, LocalTestPathKind::NewDirectory)
            .map_err(|_| "目标观察目录已存在或路径无效，拒绝覆盖")?;
        Ok(Self {
            backend: backend.to_path_buf(),
            run_dir: request.run_dir.to_path_buf(),
            manifest: inputs.manifest,
            target_dir: inputs.target_dir,
            initialized_path: inputs.initialized_path,
            initialized_binding: inputs.initialized_binding,
            initialized: inputs.initialized,
            request_path: inputs.request_path,
            request: inputs.request,
            environment: inputs.environment,
            observation_dir,
        })
    }
    pub(crate) fn target(&self) -> &Value {
        self.storage_inputs().target()
    }

    pub(crate) fn target_scope(&self) -> Result<&str> {
        self.storage_inputs().target_scope()
    }

    pub(crate) fn databases(&self) -> Result<Vec<Value>> {
        self.storage_inputs().databases()
    }

    pub(crate) fn storage(&self) -> &Value {
        self.request.get("storage").expect("已校验目标请求")
    }

    pub(crate) fn reset(&self) -> &Value {
        self.request.get("reset").expect("已校验目标请求")
    }

    pub(crate) fn execution_backend(&self) -> Result<PathBuf> {
        match self
            .request
            .get("execution_backend")
            .filter(|value| !value.is_null())
        {
            Some(value) => Ok(PathBuf::from(
                value
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("目标执行工作树缺少路径")?,
            )),
            None => Ok(self.backend.clone()),
        }
    }

    pub(crate) fn tool(&self, name: &str) -> Result<PathBuf> {
        self.storage_inputs().tool(name)
    }
}

struct Inputs {
    manifest: Value,
    target_dir: PathBuf,
    initialized_path: PathBuf,
    initialized_binding: Value,
    initialized: Value,
    request_path: PathBuf,
    request: Value,
    environment: BTreeMap<String, String>,
}

/// 生命周期 preflight 尚未签发 attempt；这里只读取并核对输入，不创建观察目录。
pub(crate) fn validate_inputs(backend: &Path, stage: &StageRequest<'_>) -> Result<()> {
    read_inputs(backend, stage)?;
    Ok(())
}

fn read_inputs(backend: &Path, stage: &StageRequest<'_>) -> Result<Inputs> {
    if stage.stage != "target-verify" || stage.mode != "run" {
        return Err("target-verify 只支持首次 run，不接受 reconcile 或 resume".into());
    }
    let loaded = manifest::load(backend, &stage.run_dir.join("manifest.json"))?;
    let initialized_binding = loaded
        .value
        .get("initialized")
        .ok_or("复制运行清单缺少初始化收据")?
        .clone();
    let initialized_path =
        manifest::bound_file(backend, &initialized_binding, "目标初始化收据", true)?;
    let target_dir = initialized_path
        .parent()
        .ok_or("初始化收据缺少父目录")?
        .to_path_buf();
    validate_local_test_path(&target_dir, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "目标证据目录无效")?;
    let initialized = json_io::read(&initialized_path)?;
    validate_initialized(&initialized)?;
    let prepare_path = target_dir.join("prepare.json");
    if initialized["prepare_sha256"].as_str() != Some(digest::file(&prepare_path)?.sha256.as_str())
    {
        return Err("初始化收据与 prepare.json 摘要不同".into());
    }
    let prepare = json_io::read(&prepare_path)?;
    let request_descriptor = prepare
        .get("request")
        .ok_or("prepare.json 缺少目标请求绑定")?;
    let request_path = manifest::bound_file(backend, request_descriptor, "目标请求", true)?;
    let request_value = json_io::read(&request_path)?;
    request_evidence::validate_prepare(
        backend,
        &target_dir,
        &prepare,
        &initialized,
        &request_value,
    )?;
    if json_io::read(&target_dir.join("request.json"))? != request_value {
        return Err("目标 request.json 与 prepare 绑定不同".into());
    }
    if json_io::read(&target_dir.join("initialized-candidate.json"))? != initialized {
        return Err("目标 initialized-candidate.json 与已发布初始化收据不同".into());
    }
    let binding = request::validate(backend, &request_value)?;
    let environment = load_environment(
        backend,
        loaded
            .value
            .get("target_environment")
            .ok_or("复制运行清单缺少目标环境")?,
    )?;
    let generation =
        request_configuration::validate(backend, &request_value, &binding, &environment)?;
    validate_generation_inputs(&initialized, &request_value, &generation)?;
    manifest::bound_file(backend, &initialized_binding, "目标初始化收据", true)?;
    manifest::bound_file(backend, request_descriptor, "目标请求", true)?;
    if manifest::load(backend, &stage.run_dir.join("manifest.json"))?.digest != loaded.digest {
        return Err("目标输入核验期间复制运行清单变化".into());
    }
    Ok(Inputs {
        manifest: loaded.value,
        target_dir,
        initialized_path,
        initialized_binding,
        initialized,
        request_path,
        request: request_value,
        environment,
    })
}

fn validate_generation_inputs(
    initialized: &Value,
    request: &Value,
    generation: &Value,
) -> Result<()> {
    for (field, expected) in json_io::object(generation, "当前静态 generation")? {
        if initialized["generation"].get(field) != Some(expected) {
            return Err(format!("目标 generation 静态绑定变化：{field}").into());
        }
    }
    if initialized.get("id") != request.get("id")
        || initialized.get("scope_id") != request["target"].get("scope_id")
    {
        return Err("初始化请求 ID 或 scope 不同".into());
    }
    Ok(())
}

fn validate_initialized(value: &Value) -> Result<()> {
    exact(
        value,
        &[
            "format_version",
            "status",
            "id",
            "scope_id",
            "completed_at",
            "generation",
            "prepare_sha256",
            "reset",
            "inventory",
            "objects",
            "redis",
            "history",
            "controlled_generation_never_started",
            "external_writers_discovered",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ],
        "目标初始化收据",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["status"].as_str() != Some("fresh_target_initialized")
        || [
            "target_ready",
            "clone_verified",
            "restore_qualified",
            "external_writers_discovered",
        ]
        .iter()
        .any(|field| value[field].as_bool() != Some(false))
        || value["controlled_generation_never_started"].as_bool() != Some(true)
    {
        return Err("目标初始化收据类型、状态或资格字段不符".into());
    }
    Ok(())
}

fn load_environment(backend: &Path, descriptor: &Value) -> Result<BTreeMap<String, String>> {
    let path = manifest::bound_file(backend, descriptor, "目标环境", true)?;
    let value = json_io::read(&path)?;
    let root = exact(&value, &["environment"], "目标环境")?;
    json_io::object(root.get("environment").unwrap(), "目标环境变量")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                value
                    .as_str()
                    .ok_or_else(|| format!("目标环境变量 {key} 必须为字符串"))?
                    .to_owned(),
            ))
        })
        .collect()
}

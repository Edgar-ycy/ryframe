//! 将 fresh target 配置绑定到独立的正式运行登记。
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{check, inputs::Inputs, role_configuration::RoleConfiguration};
use crate::{
    Result,
    ci::databases::files::private_directory,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::target_verify::input::request_configuration::canonical_path,
        inputs::{
            document::{self, Document},
            target::TargetRequest,
        },
    },
};

const WORKER_ID: u16 = 2;
const ENVIRONMENT: &str = "environment.json";
const ROLES: &str = "roles";

#[derive(Debug, PartialEq, Eq)]
pub(in crate::recovery::runtime) struct Source {
    pub(in crate::recovery::runtime) directory: PathBuf,
    pub(in crate::recovery::runtime) sha256: String,
}

impl Source {
    pub(in crate::recovery::runtime) fn from_target(target: &TargetRequest) -> Result<Self> {
        let request = &target.document.value;
        let directory = canonical_path(Path::new(document::text(
            &request["config_dir"],
            "fresh target 配置目录",
        )?))?;
        check(
            directory.starts_with(&target.execution),
            "fresh target 配置不属于其原执行树",
        )?;
        Ok(Self {
            directory,
            sha256: document::digest(&request["configuration_sha256"], "fresh target 配置摘要")?,
        })
    }

    pub(in crate::recovery::runtime) fn publish(
        &self,
        backend: &Path,
        registration: &Path,
    ) -> Result<Value> {
        let configuration = RoleConfiguration::capture(&self.directory, &self.sha256, WORKER_ID)?;
        let directory = sidecar(registration)?;
        validate_absolute_path(&directory, LocalTestPathKind::StateDirectory)?;
        private_directory(&directory)
            .map_err(|_| "恢复运行配置目录必须是新私有目录，保留原文件")?;
        let roles = directory.join(ROLES);
        configuration.publish(&roles)?;
        let path = directory.join(ENVIRONMENT);
        publish_private(&path, &private_environment(&roles)?)?;
        let environment = document::read(&path, backend, "恢复运行私有环境")?;
        self.verify(&environment)?;
        Ok(document::binding(&environment))
    }

    pub(in crate::recovery::runtime) fn verify(&self, environment: &Document) -> Result<()> {
        let directory = environment.path.parent().ok_or("恢复运行配置缺少父目录")?;
        let roles = directory.join(ROLES);
        RoleConfiguration::capture(&self.directory, &self.sha256, WORKER_ID)?.verify(&roles)?;
        check(
            private_environment(&roles)? == environment.value,
            "恢复运行私有环境与登记角色配置不同",
        )?;
        environment.assert_unchanged()
    }
}

pub(in crate::recovery::runtime) fn sidecar(registration: &Path) -> Result<PathBuf> {
    let name = registration
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("恢复运行登记文件名无效")?;
    Ok(registration.with_file_name(format!("{name}-configuration")))
}

pub(in crate::recovery::runtime) fn read(
    inputs: &mut Inputs,
    registration: &Document,
) -> Result<Document> {
    let descriptor = &registration.value["runtime_environment"];
    validate_binding(&registration.path, descriptor)?;
    inputs.bound(descriptor)
}

pub(in crate::recovery::runtime) fn verify(
    backend: &Path,
    registration: &Path,
    descriptor: &Value,
    source: &Source,
) -> Result<()> {
    validate_binding(registration, descriptor)?;
    let (_, environment) = document::bound_file(backend, descriptor, "恢复运行私有环境")?;
    source.verify(&environment)
}

fn validate_binding(registration: &Path, descriptor: &Value) -> Result<()> {
    let expected = sidecar(registration)?.join(ENVIRONMENT);
    let binding = document::binding_shape(descriptor, "恢复运行私有环境绑定")?;
    check(
        binding.path == expected,
        "恢复运行私有环境必须位于原登记文件旁的受管目录",
    )
}

fn private_environment(roles: &Path) -> Result<Value> {
    let api = roles.join("config");
    let worker = roles.join("worker-config");
    let table = ryframe_config::AppConfig::read_document(&api, ryframe_config::Environment::Test)
        .map_err(|_| "恢复角色 TOML 配置无法安全读取")?;
    let scope = table
        .get("scope_id")
        .and_then(toml::Value::as_str)
        .ok_or("恢复角色配置缺少 scope_id")?;
    let token = table
        .get("monitor")
        .and_then(|table| table.get("metrics_bearer_token"))
        .and_then(toml::Value::as_str)
        .ok_or("恢复角色配置缺少指标令牌")?;
    Ok(json!({
        "scope_id":scope, "metrics_bearer_token":token, "api_config_dir":api, "worker_config_dir":worker,
    }))
}

fn publish_private(path: &Path, value: &Value) -> Result<()> {
    validate_absolute_path(path, LocalTestPathKind::OutputFile)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&document::canonical_bytes(value)?)?;
    file.sync_all()?;
    Ok(())
}

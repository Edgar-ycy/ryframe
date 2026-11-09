use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};
use url::Url;

use super::{artifact, canonical, directory, documents, regular};
use crate::{
    Result,
    recovery::clone::native::{
        digest, input::digest_shape,
        target_verify::input::request_configuration::python_configuration_hash,
    },
};

pub(super) fn validate(backend: &Path, product: &Path, value: &mut Value) -> Result<()> {
    documents::exact(
        value,
        &[
            "format_version",
            "backend_root",
            "scope_id",
            "configuration_sha256",
            "api_config_dir",
            "worker_config_dir",
            "api_configuration_sha256",
            "worker_configuration_sha256",
            "worker_ready_url",
            "artifacts",
        ],
        "runtime 收据",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || canonical(Path::new(documents::text(
            &value["backend_root"],
            "后端根",
        )?))?
            != product
        || !Regex::new(r"^[a-z0-9][a-z0-9-]{2,63}$")?
            .is_match(documents::text(&value["scope_id"], "scope")?)
    {
        return Err("runtime 收据不属于当前后端或隔离scope".into());
    }
    let mut selected = Vec::new();
    for (role, name) in [("api", "config"), ("worker", "worker-config")] {
        let path = PathBuf::from(documents::text(
            &value[format!("{role}_config_dir")],
            "配置目录",
        )?);
        if path.file_name().is_none_or(|part| part != name) {
            return Err("runtime 双配置目录名与登记角色不匹配".into());
        }
        let path = directory(&path, backend)?;
        let expected = digest_shape(&value[format!("{role}_configuration_sha256")])?;
        if configuration_digest(backend, &path)? != expected {
            return Err("runtime 双配置文件摘要发生变化".into());
        }
        selected.push(path);
    }
    if selected[0].parent() != selected[1].parent()
        || value["api_configuration_sha256"] == value["worker_configuration_sha256"]
        || digest_shape(&value["configuration_sha256"])?
            != python_configuration_hash(&json!([
                value["api_configuration_sha256"],
                value["worker_configuration_sha256"]
            ]))?
    {
        return Err("runtime 双配置摘要或组合摘要不匹配".into());
    }
    ready_url(&value["worker_ready_url"], true)?;
    documents::exact(&value["artifacts"], &["api", "worker"], "runtime 双产物")?;
    for role in ["api", "worker"] {
        value["artifacts"][role] = artifact(backend, &value["artifacts"][role])?;
    }
    Ok(())
}

pub(super) fn configuration_digest(backend: &Path, directory: &Path) -> Result<String> {
    // 与Python resolve(strict=True)一致，调用方的分隔符写法不进入摘要。
    let directory = super::directory(directory, backend)?;
    crate::rate_limit::load_app_table(backend, Some(&directory), "test")
        .map_err(|error| error.message().to_owned())?;
    let mut files = BTreeMap::new();
    for name in ["app.toml", "app.test.toml"] {
        let path = directory.join(name);
        if name == "app.toml" && !path.try_exists()? {
            continue;
        }
        let path = regular(&path, &directory, false)?;
        files.insert(name, digest::file(&path)?.sha256);
    }
    python_configuration_hash(&json!({"environment": {"APP_ENV": "test"},
        "config_directory": directory, "files": files}))
}

pub(super) fn ready_url(value: &Value, strict: bool) -> Result<()> {
    let raw = documents::text(value, "runtime 就绪地址")?;
    let parsed = Url::parse(raw).map_err(|_| "runtime 就绪地址无效")?;
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .ok_or("runtime 就绪地址缺少协议")?;
    let host_port = authority.rsplit('@').next().ok_or("runtime 就绪地址无效")?;
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or("runtime 就绪地址必须显式提供端口")?;
    if parsed.scheme() != "http"
        || host != "127.0.0.1"
        || port.parse::<u16>().is_err()
        || parsed.path() != "/readyz"
        || (strict
            && (authority.contains('@')
                || parsed.query().is_some_and(|v| !v.is_empty())
                || parsed.fragment().is_some_and(|v| !v.is_empty())))
    {
        return Err("runtime Worker或生产者就绪地址无效".into());
    }
    Ok(())
}

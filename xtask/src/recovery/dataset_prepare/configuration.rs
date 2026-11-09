use crate::{
    Result,
    recovery::clone::native::{s3::app_configuration, session::credentials},
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 只读取显式当前凭据；已有数据验证绝不探测初始化密码。
pub(in crate::recovery) fn current(
    root: &Path,
    dataset: &Value,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<String> {
    let descriptor = &dataset["administrator_credentials"];
    let account = &dataset["admin"];
    let tenant = account["tenant_id"].as_str().ok_or("管理员租户缺失")?;
    let username = account["username"].as_str().ok_or("管理员用户名缺失")?;
    let password = credentials::load_account(root, descriptor, tenant, username)?;
    super::super::fixture_control::environment::validate_password(&password)?;
    let path = PathBuf::from(descriptor["path"].as_str().ok_or("当前凭据路径缺失")?);
    let bytes = std::fs::read(&path)?;
    if credentials::load_account(root, descriptor, tenant, username)? != password
        || bytes.len() as u64 != descriptor["bytes"].as_u64().ok_or("当前凭据长度缺失")?
        || super::model::sha(&bytes) != descriptor["sha256"]
    {
        return Err("当前凭据读取期间变化".into());
    }
    snapshots.push((path, bytes));
    Ok(password)
}

/// 初始化密码仅在显式 fresh 准备阶段加载，不从当前凭据推断。
pub(super) fn initial(
    root: &Path,
    binding: &Value,
    current: &str,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<String> {
    let table = app_configuration::load(root, binding, snapshots)?;
    let password = table
        .get("bootstrap")
        .and_then(toml::Value::as_table)
        .and_then(|value| value.get("admin_password"))
        .and_then(toml::Value::as_str)
        .ok_or("绑定配置缺少 bootstrap.admin_password，必须显式提供初始化密码")?;
    super::super::fixture_control::environment::validate_password(password)?;
    if password == current {
        return Err("初始化密码与显式当前密码必须不同".into());
    }
    Ok(password.to_owned())
}

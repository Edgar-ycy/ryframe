//! 复制后管理员使用独立绑定的当前凭据；不推断初始化密码或修改账号。
use crate::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const REJECTED: &str = "管理员凭据校验失败；检查后端 .local-tests 内的 TOML 路径、文件绑定及 administrator.tenant_id/username/password；password 必须是首次改密后的当前凭据";

pub(crate) fn load(root: &Path, descriptor: &Value, identity: &Value) -> Result<String> {
    read(root, descriptor, identity).map_err(|_| REJECTED.into())
}

fn read(root: &Path, descriptor: &Value, identity: &Value) -> Result<String> {
    crate::local_test_path::validate_absolute_path(
        root,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    super::model::identity(identity)?;
    super::super::json::exact(descriptor, &["path", "bytes", "sha256"], "管理员凭据绑定")?;
    if descriptor["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes == 0 || bytes > 64 * 1024)
    {
        return Err(REJECTED.into());
    }
    let path = super::super::lifecycle::manifest::bound_file(root, descriptor, "管理员凭据", true)?;
    let before = fs::read(&path)?;
    if path.extension().and_then(|value| value.to_str()) != Some("toml")
        || Some(before.len() as u64) != descriptor["bytes"].as_u64()
        || Some(hex::encode(Sha256::digest(&before)).as_str()) != descriptor["sha256"].as_str()
    {
        return Err(REJECTED.into());
    }
    let table: toml::Table = toml::from_str(std::str::from_utf8(&before)?)?;
    if table.len() != 1 {
        return Err(REJECTED.into());
    }
    let administrator = table
        .get("administrator")
        .and_then(toml::Value::as_table)
        .ok_or(REJECTED)?;
    if administrator.len() != 3
        || !["tenant_id", "username", "password"]
            .iter()
            .all(|key| administrator.contains_key(*key))
        || administrator.get("tenant_id").and_then(toml::Value::as_str)
            != identity["tenant_id"].as_str()
        || administrator.get("username").and_then(toml::Value::as_str)
            != identity["username"].as_str()
    {
        return Err(REJECTED.into());
    }
    let password = administrator
        .get("password")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or(REJECTED)?
        .to_owned();
    if super::super::lifecycle::manifest::bound_file(root, descriptor, "管理员凭据", true)? != path
        || fs::read(&path)? != before
    {
        return Err(REJECTED.into());
    }
    Ok(password)
}

/// 现存 Python 注册调用方只请求校验；协议结果不返回明文凭据。
pub(crate) fn validate_request(args: &Value) -> Result<Value> {
    let result = (|| -> Result<Value> {
        super::super::json::exact(
            args,
            &["backend", "identity", "administrator_credentials"],
            "管理员凭据请求",
        )?;
        let root = Path::new(args["backend"].as_str().ok_or(REJECTED)?);
        load(root, &args["administrator_credentials"], &args["identity"])?;
        Ok(Value::Null)
    })();
    result.map_err(|_| REJECTED.into())
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;

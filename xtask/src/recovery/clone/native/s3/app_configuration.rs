//! 显式绑定的应用配置文件；密钥不进入公开计划、协议响应和诊断。
use super::super::json as documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn load(
    root: &Path,
    binding: &Value,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<toml::Table> {
    documents::exact(
        binding,
        &["app_env", "directory", "files", "sha256"],
        "应用配置绑定",
    )?;
    let app_env = binding["app_env"]
        .as_str()
        .filter(|value| matches!(*value, "dev" | "test"))
        .ok_or("隔离应用配置环境只允许dev或test")?;
    let directory = binding["directory"]
        .as_str()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(PathBuf::from)
        .ok_or("应用配置绑定缺少目录")?;
    validate_local_test_path(&directory, root, LocalTestPathKind::ExistingDirectory)?;
    let environment = format!("app.{app_env}.toml");
    let mut files = serde_json::Map::new();
    for (index, name) in ["app.toml", environment.as_str()].iter().enumerate() {
        let file = directory.join(name);
        if index == 0 && !file.exists() {
            continue;
        }
        validate_local_test_path(&file, root, LocalTestPathKind::ExistingFile)?;
        let bytes = fs::read(&file).map_err(|_| "无法读取绑定的应用配置文件")?;
        if bytes.len() > 1024 * 1024 {
            return Err("应用配置文件超过1MiB".into());
        }
        files.insert(
            (*name).into(),
            json!({"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}),
        );
        snapshots.push((file, bytes));
    }
    let expected = json!({"app_env":app_env,"directory":directory,"files":files});
    let sha = hex::encode(Sha256::digest(serde_json::to_vec(&expected)?));
    if binding["files"] != expected["files"] || binding["sha256"] != sha {
        return Err("应用配置绑定摘要已变化；重新审阅计划，禁止自动重放".into());
    }
    let table = crate::rate_limit::load_app_table(root, Some(&directory), app_env)
        .map_err(|_| "绑定的app配置加载失败；检查对应环境文件、字段与类型；诊断不回显配置内容")?;
    unchanged(snapshots)?;
    Ok(table)
}
pub(crate) fn unchanged(snapshots: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    for (file, bytes) in snapshots {
        if fs::read(file).map_err(|_| "无法复核绑定应用配置")? != *bytes {
            return Err("读取期间绑定的应用配置文件发生变化".into());
        }
    }
    Ok(())
}

impl super::Configuration {
    /// 与 read 分离的显式文件绑定入口；调用方必须主动选择该契约。
    pub(crate) fn read_app_binding(root: &Path, value: &Value) -> Result<Self> {
        documents::exact(
            value,
            &["endpoint", "region", "configuration"],
            "参考对象配置绑定",
        )?;
        let mut snapshots = Vec::new();
        let table = load(root, &value["configuration"], &mut snapshots)?;
        let objects = table
            .get("object_storage")
            .and_then(toml::Value::as_table)
            .ok_or("绑定app配置缺少object_storage")?;
        if objects
            .keys()
            .any(|name| matches!(name.as_str(), "session_token" | "security_token"))
        {
            return Err("对象配置不接受会话令牌".into());
        }
        let field = |name| {
            objects
                .get(name)
                .and_then(toml::Value::as_str)
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
                .ok_or("绑定app配置缺少有效的object_storage凭据字段")
        };
        let configuration = Self::read(
            &json!({"endpoint":value["endpoint"],"region":value["region"],"access_key":field("access_key")?,"secret_key":field("secret_key")?}),
        )?;
        unchanged(&snapshots)?;
        Ok(configuration)
    }
}

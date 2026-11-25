//! 显式绑定的应用配置文件；密钥不进入公开计划、协议响应和诊断。
use super::super::json as documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn linked(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        linked || metadata.file_attributes() & REPARSE_POINT != 0
    };
    Ok(linked)
}

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
    let project_config = root.join("config");
    if directory == project_config {
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || linked(&directory)? {
            return Err("应用配置目录必须是项目config或受管.local-tests普通目录".into());
        }
    } else {
        validate_local_test_path(&directory, root, LocalTestPathKind::ExistingDirectory)?;
    }
    let environment = format!("app.{app_env}.toml");
    let mut files = serde_json::Map::new();
    for (index, name) in ["app.toml", environment.as_str()].iter().enumerate() {
        let file = directory.join(name);
        crate::recovery::runtime::dataset_authority::reads::optional(&file)?;
        if index == 0 && !file.exists() {
            continue;
        }
        if directory == project_config {
            let metadata = fs::symlink_metadata(&file)?;
            if !metadata.is_file() || linked(&file)? {
                return Err("应用配置文件必须是普通文件".into());
            }
        } else {
            validate_local_test_path(&file, root, LocalTestPathKind::ExistingFile)?;
        }
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

/// 仅供内存比较和传输；不得将返回的凭据写入来源清单或公开收据。
pub(crate) fn private_configuration(root: &Path, binding: &Value) -> Result<Value> {
    let mut snapshots = Vec::new();
    let table = load(root, binding, &mut snapshots)?;
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
    let mut value = serde_json::Map::new();
    for name in ["endpoint", "region", "access_key", "secret_key"] {
        let field = objects
            .get(name)
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .ok_or("绑定app配置缺少有效的object_storage字段；请修正配置文件")?;
        value.insert(name.into(), json!(field));
    }
    let value = Value::Object(value);
    super::Configuration::read(&value)?;
    unchanged(&snapshots)?;
    Ok(value)
}

pub(crate) fn public_configuration(root: &Path, binding: &Value) -> Result<Value> {
    let value = private_configuration(root, binding)?;
    Ok(json!({"endpoint":value["endpoint"],"region":value["region"]}))
}

impl super::Configuration {
    pub(crate) fn from_app_binding(root: &Path, binding: &Value) -> Result<Self> {
        Self::read(&private_configuration(root, binding)?)
    }

    pub(crate) fn read_app_binding(root: &Path, value: &Value) -> Result<Self> {
        documents::exact(
            value,
            &["endpoint", "region", "configuration"],
            "参考对象配置绑定",
        )?;
        let objects = private_configuration(root, &value["configuration"])?;
        if value["endpoint"] != objects["endpoint"] || value["region"] != objects["region"] {
            return Err("对象连接身份与绑定配置文件不同；请重新生成来源投影".into());
        }
        Self::read(&objects)
    }
}

pub(crate) fn configuration_value(
    backend: &Path,
    directory: &Path,
    app_env: &str,
) -> Result<Value> {
    if !matches!(app_env, "dev" | "test") {
        return Err("参考配置环境只允许dev或test".into());
    }
    validate_absolute_path(directory, LocalTestPathKind::ExistingDirectory)?;
    let backend =
        super::super::target_verify::input::request_configuration::canonical_path(backend)?;
    let directory =
        super::super::target_verify::input::request_configuration::canonical_path(directory)?;
    let local = backend.join(".local-tests");
    if !directory.is_dir()
        || linked(&directory)?
        || directory != backend.join("config") && !directory.starts_with(local)
    {
        return Err("参考配置目录必须是项目config或后端受管.local-tests目录".into());
    }
    let environment = format!("app.{app_env}.toml");
    let mut files = serde_json::Map::new();
    for (index, name) in ["app.toml", environment.as_str()].iter().enumerate() {
        let file = directory.join(name);
        if index == 0 && !file.exists() {
            continue;
        }
        if !file.is_file() || linked(&file)? {
            return Err("参考配置文件缺失或经过链接".into());
        }
        files.insert((*name).to_owned(), {
            let digest = super::super::digest::file(&file)?;
            json!({"bytes": digest.bytes, "sha256": digest.sha256})
        });
    }
    let value = json!({"app_env":app_env,"directory":directory,"files":files});
    let sha = super::super::digest::plan_hash(&value)?;
    Ok(json!({"app_env":app_env,"directory":directory,"files":value["files"],"sha256":sha}))
}

pub(crate) fn runtime_configuration(
    coordinator: &Path,
    execution: &Path,
    environment: &BTreeMap<String, String>,
    runtime: &Value,
    role: &str,
) -> Result<(Value, BTreeMap<String, Value>)> {
    use super::super::{
        resources::common,
        seed::runtime_model,
        target_verify::input::request_configuration::{
            canonical_path, configuration_files, configuration_identity,
        },
    };

    runtime_model::validate_api_environment(coordinator, environment)?;
    let declared = runtime
        .get(format!("{role}_config_dir"))
        .ok_or("源运行收据缺少角色配置目录")?;
    let path = Path::new(documents::text(declared, "源角色配置目录")?);
    let root = if coordinator != execution
        && path.is_absolute()
        && !canonical_path(path)?.starts_with(canonical_path(execution)?)
    {
        common::local_directory(coordinator, path, "协调根的已登记源角色配置目录")?;
        coordinator
    } else {
        execution
    };
    let (directory, files) = configuration_files(root, environment, Some(declared))?;
    let sha = configuration_identity(&directory, &files)?;
    if runtime[format!("{role}_configuration_sha256")].as_str() != Some(sha.as_str()) {
        return Err("源 TOML 配置与运行收据摘要不同；请核对绑定文件".into());
    }
    let table = crate::rate_limit::load_app_table(execution, Some(&directory), "test")
        .map_err(|_| "源 TOML 配置无法加载；请检查所选环境文件与字段类型")?;
    if configuration_files(root, environment, Some(declared))? != (directory, files.clone()) {
        return Err("源 TOML 配置在读取期间变化".into());
    }
    Ok((serde_json::to_value(table)?, files))
}

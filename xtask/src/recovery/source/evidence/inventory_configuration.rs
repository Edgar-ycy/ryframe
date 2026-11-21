use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use super::{binding, documents};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{
        digest,
        target_verify::input::request_configuration::{canonical_path, configuration_file_digests},
    },
};

const DIRECTORY_NOT_ALLOWED: &str = "库存配置必须位于明确后端内且不能经过链接";
const DIRECTORY_REQUIRED: &str = "库存配置目录必须存在";

#[derive(Debug)]
pub(super) struct PathSyntaxError(pub(super) &'static str);

impl std::fmt::Display for PathSyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for PathSyntaxError {}

pub(super) fn request(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "evidence_root", "config_dir", "app_env"],
        "库存配置快照参数",
    )?;
    let backend = existing_directory(&binding::path(&args["backend"])?)?;
    let evidence_root = if args["evidence_root"].is_null() {
        None
    } else {
        Some(existing_directory(&binding::path(&args["evidence_root"])?)?)
    };
    let directory = existing_directory(&directory_path(&backend, &args["config_dir"])?)?;
    if !directory.starts_with(&backend)
        && !evidence_root.is_some_and(|root| directory.starts_with(root))
    {
        return Err(DIRECTORY_NOT_ALLOWED.into());
    }
    let app_env = args["app_env"]
        .as_str()
        .filter(|app_env| matches!(*app_env, "dev" | "test" | "prod"))
        .ok_or("库存 APP_ENV 必须明确选择 dev、test 或 prod")?;
    // 按实际文件系统核验已选文件，包括 Windows 对名称大小写的处理。
    validate_absolute_path(
        &directory.join(format!("app.{app_env}.toml")),
        LocalTestPathKind::ExistingFile,
    )
    .map_err(|_| "库存配置缺少无链接的已选环境文件")?;
    let files = configuration_file_digests(&directory, is_toml).map_err(|error| {
        let message = error.to_string();
        if message.starts_with("复制证据必须是普通文件：")
            || message.starts_with("复制证据打开后不是普通文件：")
        {
            "库存配置文件不能经过链接".into()
        } else {
            error
        }
    })?;
    if files.is_empty() {
        return Err("库存配置目录没有 TOML 文件".into());
    }
    Ok(json!({
        "directory": digest::path_text(&directory, "库存配置目录")?,
        "files": files,
        "environment_sha256": digest::plan_hash(&json!({"APP_ENV": app_env}))?,
    }))
}

fn directory_path(backend: &Path, config_dir: &Value) -> Result<PathBuf> {
    if config_dir.is_null() {
        return Ok(backend.join("config"));
    }
    let raw = config_dir
        .as_str()
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or(PathSyntaxError("库存配置目录必须为明确文本路径"))?;
    let directory = Path::new(raw);
    if raw.chars().any(char::is_control)
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err(PathSyntaxError("库存配置目录不能包含控制字符或父目录跳转").into());
    }
    Ok(if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        backend.join(directory)
    })
}

fn existing_directory(path: &Path) -> Result<PathBuf> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory).map_err(|error| {
        if error.starts_with("路径不能经过符号链接或 junction：") {
            DIRECTORY_NOT_ALLOWED.to_owned()
        } else if matches!(
            error.as_str(),
            "输入路径不存在" | "输入路径必须是现有普通目录"
        ) || error.starts_with("路径的现有父级必须是目录：")
        {
            DIRECTORY_REQUIRED.to_owned()
        } else {
            error
        }
    })?;
    let directory = canonical_path(path)?;
    #[cfg(windows)]
    if let Some(share) = directory
        .to_str()
        .and_then(|name| name.strip_prefix(r"UNC\"))
    {
        return Ok(PathBuf::from(format!(r"\\{share}")));
    }
    if !directory.is_absolute() {
        return Err("库存配置目录必须为规范绝对路径".into());
    }
    Ok(directory)
}

fn is_toml(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.as_encoded_bytes();
    if cfg!(windows) {
        name.last_chunk::<5>()
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b".toml"))
    } else {
        name.ends_with(b".toml")
    }
}

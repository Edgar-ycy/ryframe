//! 私有配置发布复用现有构造和数据库凭据核验，不启动工具或修改进程环境。
use super::super::protocol::GuardError;
use super::{configuration, files};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn configuration(
    execution: &Path,
    output: &Path,
    settings: &Value,
    credentials: &Value,
) -> Result<()> {
    let base = execution.join("config/app.toml");
    validate_absolute_path(&base, LocalTestPathKind::ExistingFile)?;
    let before = files::binding(&base)?;
    let bytes = fs::read(&base)?;
    if settings["targets"].as_array().is_none_or(Vec::is_empty) {
        return Err(Box::new(GuardError("夹具配置必须声明租户数据目标")));
    }
    let directory = output.join("config");
    crate::ci::databases::files::private_directory(&directory)
        .map_err(|_| GuardError("无法创建私有夹具配置目录"))?;
    files::write_private(&directory.join("app.toml"), &bytes)?;
    let text = configuration::text(settings)?;
    let mut args = credentials.clone();
    args["output"] = json!(directory.join("app.test.toml"));
    args["text"] = text["text"].clone();
    crate::recovery::source::evidence::binding::configuration(&args)?;
    files::private_permissions(&directory.join("app.test.toml"), false)?;
    if files::binding(&base)? != before || fs::read(base)? != bytes {
        return Err(Box::new(GuardError("基础配置在复制期间变化；保留本地现场")));
    }
    Ok(())
}

pub(super) fn digest(execution: &Path, app_env: &str, directory: &Path) -> Result<String> {
    if app_env != "test" {
        return Err(Box::new(GuardError("夹具配置必须明确为 test")));
    }
    crate::rate_limit::load_app_table(execution, Some(directory), app_env)
        .map_err(|_| GuardError("夹具配置无法安全解析；检查 app.toml/app.test.toml"))?;
    crate::recovery::fixture_control::artifact::runtime::configuration_digest(directory)
}

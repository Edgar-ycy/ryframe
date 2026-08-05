mod spec;
mod table;
mod value;

use std::{fs, path::Path};

use ryframe_kernel::{AppError, AppResult};

use self::spec::ENV_OVERRIDES;

pub(super) fn apply_env_overrides(table: &mut toml::Table) -> AppResult<()> {
    for spec in ENV_OVERRIDES {
        let direct_value = read_env(spec.name)?;
        let file_name = format!("{}_FILE", spec.name);
        let file_path = if spec.allow_file {
            read_env(&file_name)?
        } else {
            None
        };
        if direct_value.is_some() && file_path.is_some() {
            return Err(AppError::Config(format!(
                "环境变量 {} 与 {} 不能同时设置",
                spec.name, file_name
            )));
        }
        let (raw_value, from_file) = match (direct_value, file_path) {
            (Some(value), None) => (value, false),
            (None, Some(path)) => (read_override_file(&file_name, &path)?, true),
            (None, None) => continue,
            (Some(_), Some(_)) => unreachable!("已在前置校验中拒绝重复配置来源"),
        };
        if raw_value.is_empty() && from_file {
            return Err(AppError::Config(format!(
                "环境变量 {} 指向的配置文件不能为空",
                file_name
            )));
        }
        let parsed_value = value::parse(spec.name, &raw_value, spec.value_type)?;
        table::insert(table, spec.path, parsed_value);
    }

    Ok(())
}

fn read_env(name: &str) -> AppResult<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(AppError::Config(format!(
            "环境变量 {name} 必须使用有效的 Unicode 编码"
        ))),
    }
}

fn read_override_file(variable_name: &str, raw_path: &str) -> AppResult<String> {
    let path = raw_path.trim();
    if path.is_empty() {
        return Err(AppError::Config(format!(
            "环境变量 {variable_name} 不能是空路径"
        )));
    }
    let bytes = fs::read(Path::new(path)).map_err(|error| {
        AppError::Config(format!(
            "无法读取环境变量 {variable_name} 指向的配置文件: {error}"
        ))
    })?;
    let mut value = String::from_utf8(bytes).map_err(|_| {
        AppError::Config(format!(
            "环境变量 {variable_name} 指向的配置文件必须使用 UTF-8 编码"
        ))
    })?;
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    Ok(value)
}

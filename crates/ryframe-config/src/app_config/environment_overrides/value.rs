use ryframe_kernel::{AppError, AppResult};

use super::spec::EnvValueType;

pub(super) fn parse(name: &str, value: &str, value_type: EnvValueType) -> AppResult<toml::Value> {
    match value_type {
        EnvValueType::String => Ok(toml::Value::String(value.to_string())),
        EnvValueType::Integer => value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效整数: {}", name, e))),
        EnvValueType::Float => value
            .parse::<f64>()
            .map(toml::Value::Float)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效小数: {}", name, e))),
        EnvValueType::Bool => value
            .parse::<bool>()
            .map(toml::Value::Boolean)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效布尔值: {}", name, e))),
        EnvValueType::StringArray => {
            let values = value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| toml::Value::String(item.to_string()))
                .collect();
            Ok(toml::Value::Array(values))
        }
        EnvValueType::Json => {
            let json = serde_json::from_str::<serde_json::Value>(value).map_err(|error| {
                AppError::Config(format!("环境变量 {name} 不是有效 JSON: {error}"))
            })?;
            toml::Value::try_from(json).map_err(|error| {
                AppError::Config(format!("环境变量 {name} 无法转换为配置值: {error}"))
            })
        }
    }
}

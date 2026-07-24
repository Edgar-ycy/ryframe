use ryframe_common::{AppError, AppResult};

use super::spec::EnvValueType;

pub(super) fn parse(name: &str, value: &str, value_type: EnvValueType) -> AppResult<toml::Value> {
    match value_type {
        EnvValueType::String => Ok(toml::Value::String(value.to_string())),
        EnvValueType::Integer => value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效整数: {}", name, e))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_arrays_are_trimmed_and_empty_items_are_ignored() {
        let parsed = parse(
            "APP_CORS_ALLOW_ORIGINS",
            " https://one.example, ,https://two.example ",
            EnvValueType::StringArray,
        )
        .unwrap();

        assert_eq!(
            parsed,
            toml::Value::Array(vec![
                toml::Value::String("https://one.example".into()),
                toml::Value::String("https://two.example".into()),
            ])
        );
    }

    #[test]
    fn invalid_values_keep_the_existing_error_context() {
        let integer_error = parse("APP_APP_PORT", "not-a-port", EnvValueType::Integer)
            .unwrap_err()
            .to_string();
        let bool_error = parse("APP_RATE_LIMIT_ENABLED", "yes", EnvValueType::Bool)
            .unwrap_err()
            .to_string();
        let json_error = parse("APP_DATABASE_REPLICAS", "{", EnvValueType::Json)
            .unwrap_err()
            .to_string();

        assert!(integer_error.contains("环境变量 APP_APP_PORT 不是有效整数:"));
        assert!(bool_error.contains("环境变量 APP_RATE_LIMIT_ENABLED 不是有效布尔值:"));
        assert!(json_error.contains("环境变量 APP_DATABASE_REPLICAS 不是有效 JSON:"));
    }
}

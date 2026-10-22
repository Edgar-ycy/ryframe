use ryframe_kernel::{AppError, AppResult};

pub(super) const MIN_PRODUCTION_JWT_SECRET_BYTES: usize = 32;

pub(super) fn reject_removed_secret_encoding(table: &toml::Table) -> AppResult<()> {
    inspect_removed_secret_encoding(&toml::Value::Table(table.clone()), "")
}

fn inspect_removed_secret_encoding(value: &toml::Value, path: &str) -> AppResult<()> {
    match value {
        toml::Value::String(value) if value.starts_with("ENC[") => Err(AppError::Config(format!(
            "配置 {path} 使用了已删除的 ENC[...] 格式；请直接在对应 TOML 字段填写明文值"
        ))),
        toml::Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                inspect_removed_secret_encoding(child, &child_path)?;
            }
            Ok(())
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_removed_secret_encoding(child, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

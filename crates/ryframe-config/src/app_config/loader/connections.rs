use ryframe_kernel::{AppError, AppResult};

use crate::DbConnection;

// flatten 会丢失连接内部的错误路径；先以同一个连接类型校验，不复制字段或默认值。
pub(super) fn validate(table: &toml::Table) -> AppResult<()> {
    let Some(database) = table.get("database").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for collection in ["replicas", "sources"] {
        let Some(entries) = database.get(collection).and_then(toml::Value::as_array) else {
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            let Some(connection) = entry.as_table() else {
                continue;
            };
            let mut connection = connection.clone();
            // name 属于外层命名条目，仍交给原 Source/Replica 反序列化校验。
            connection.remove("name");
            let result: Result<DbConnection, toml::de::Error> =
                toml::Value::Table(connection).try_into();
            if let Err(error) = result {
                return Err(connection_error(collection, index, error));
            }
        }
    }
    Ok(())
}

fn connection_error(collection: &str, index: usize, mut error: toml::de::Error) -> AppError {
    error.set_input(None);
    let message = error.message();
    let rendered = error.to_string();
    let (field, reason) = if let Some(rest) = message.strip_prefix("missing field `") {
        (rest.split('`').next(), "缺少必填连接字段")
    } else if let Some(rest) = message.strip_prefix("unknown field `") {
        (rest.split('`').next(), "存在未知连接字段")
    } else {
        // 去除输入后 Display 仅保留原因和 in `字段`；原因不进入最终错误。
        let field = rendered
            .strip_prefix(message)
            .map(str::trim)
            .and_then(|path| path.strip_prefix("in `"))
            .and_then(|path| path.strip_suffix('`'));
        let reason = if message.starts_with("invalid type:") {
            "连接字段类型不正确"
        } else if message.starts_with("invalid value:") {
            "连接字段值超出允许范围"
        } else if message.starts_with("unknown variant ") {
            "连接字段枚举值不受支持"
        } else {
            "连接字段不符合 DbConnection 定义"
        };
        (field, reason)
    };
    let mut path = format!("database.{collection}[{index}]");
    if let Some(field) = field.filter(|field| !field.is_empty()) {
        path.push('.');
        path.push_str(field);
    }
    AppError::Config(format!(
        "{path}：{reason}；请按 DbConnection 的字段类型、必填项、允许值和默认值修正 TOML"
    ))
}

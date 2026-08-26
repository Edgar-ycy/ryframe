use ryframe_kernel::{AppError, AppResult};

/// 按调用方提供的稳定文案解析整数 ID。
pub(crate) fn parse_id(value: &str, invalid_message: &str) -> AppResult<i64> {
    value
        .parse::<i64>()
        .map_err(|_| AppError::Validation(invalid_message.to_owned()))
}

/// 解析大于零的整数 ID，并保留调用方已有的校验文案。
pub(crate) fn parse_positive_id(value: &str, invalid_message: &str) -> AppResult<i64> {
    parse_id(value, invalid_message).and_then(|id| {
        (id > 0)
            .then_some(id)
            .ok_or_else(|| AppError::Validation(invalid_message.to_owned()))
    })
}

/// 解析可选整数 ID；缺省值与空白字符串都返回 `None`。
pub(crate) fn parse_optional_id(value: Option<&str>, label: &str) -> AppResult<Option<i64>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| AppError::Validation(format!("无效的{label}: {value}")))
}

/// 解析可选的正整数 ID；空字符串是否代表缺省由调用方在传入前决定。
pub(crate) fn parse_optional_positive_id(
    value: Option<&str>,
    invalid_message: &str,
) -> AppResult<Option<i64>> {
    value
        .map(|value| parse_positive_id(value, invalid_message))
        .transpose()
}

/// 按输入顺序解析正整数 ID 列表。
pub(crate) fn parse_positive_id_list(ids: &[String], invalid_message: &str) -> AppResult<Vec<i64>> {
    ids.iter()
        .map(|id| parse_positive_id(id, invalid_message))
        .collect()
}

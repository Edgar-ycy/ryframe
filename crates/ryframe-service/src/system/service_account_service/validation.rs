use super::*;

pub(super) fn validate_code(value: String) -> AppResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(AppError::Validation(
            "服务账号代码只能包含小写字母、数字、连字符和下划线，且最长 64 字符".into(),
        ));
    }
    Ok(value)
}

pub(super) fn required_text(value: String, field: &str, max: usize) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(AppError::Validation(format!(
            "{field}不能为空且不能超过 {max} 个字符"
        )));
    }
    Ok(value.to_owned())
}

pub(super) fn optional_text(value: Option<String>, max: usize) -> AppResult<Option<String>> {
    value
        .map(|value| required_text(value, "说明", max))
        .transpose()
}

pub(super) fn validate_rate_limit(value: i32) -> AppResult<()> {
    if !(1..=10_000).contains(&value) {
        return Err(AppError::Validation(
            "每分钟请求上限必须在 1 到 10000 之间".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_idempotency_key(value: String) -> AppResult<String> {
    let value = value.trim();
    if value.len() < 16 || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 16 到 128 个可见 ASCII 字符".into(),
        ));
    }
    Ok(value.to_owned())
}

pub(super) fn request_fingerprint(parts: &[&[u8]]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(FINGERPRINT_DOMAIN);
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().to_vec()
}

pub(super) fn unkeyed_hash(domain: &[u8], value: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    digest.finalize().to_vec()
}

pub(super) fn ensure_same_fingerprint(existing: &[u8], requested: &[u8]) -> AppResult<()> {
    if existing == requested {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "相同 Idempotency-Key 已用于不同请求".into(),
        ))
    }
}

pub(super) async fn database_now<C>(db: &C) -> AppResult<DateTime<Utc>>
where
    C: sea_orm::ConnectionTrait,
{
    DataRetentionRepository.database_utc_now(db).await
}

pub(super) fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

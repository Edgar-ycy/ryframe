use super::{super::json as document, model};
use crate::Result;
use serde_json::Value;

#[derive(Debug)]
pub(super) struct ControlFailure(pub(super) Option<i32>);

#[derive(Debug)]
pub(super) struct RemoteFailure {
    pub code: Option<i32>,
    pub diagnostic: super::diagnostic::Diagnostic,
}
impl std::fmt::Display for RemoteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}；原始退出码 {:?}；禁止重放",
            self.diagnostic, self.code
        )
    }
}
impl std::error::Error for RemoteFailure {}

impl std::fmt::Display for ControlFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Redis 精确身份操作失败，原始退出码 {:?}；保留现场，禁止重放",
            self.0
        )
    }
}

impl std::error::Error for ControlFailure {}

pub(super) fn validate_response(bytes: &[u8], identity: &Value) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 16384 {
        return Err("缓存进程响应为空或超过16KiB".into());
    }
    let value = document::parse_fragment(std::str::from_utf8(bytes)?, "缓存精确进程响应")?;
    document::exact(
        &value,
        &["identity", "alive", "boot_id", "terminated"],
        "缓存进程响应",
    )?;
    if value["identity"] != *identity
        || value["alive"].as_bool().is_none()
        || value["terminated"].as_bool().is_none()
        || !model::boot(&value["boot_id"])?
    {
        return Err("缓存控制返回其他 Redis 进程身份".into());
    }
    Ok(value)
}

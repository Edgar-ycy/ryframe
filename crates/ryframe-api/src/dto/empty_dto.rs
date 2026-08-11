use serde::Deserialize;
use utoipa::ToSchema;

/// 不携带业务字段的写操作请求体。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequestDto {}

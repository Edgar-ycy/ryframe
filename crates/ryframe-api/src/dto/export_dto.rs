use serde::Deserialize;
use utoipa::ToSchema;

/// 取消导出任务的显式命令体。
///
/// 保留空对象而不是省略请求体，使写操作契约保持一致，并为后续增加取消原因等字段预留空间。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelExportJobDto {}

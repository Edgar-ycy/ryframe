use serde::Deserialize;
use utoipa::ToSchema;

/// 资源导出的筛选条件快照。
///
/// 各资源沿用其列表接口的筛选字段，服务端会按资源类型严格反序列化并校验。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ExportRequestDto(pub serde_json::Value);

/// 取消导出任务的显式命令体。
///
/// 保留空对象而不是省略请求体，使写操作契约保持一致，并为后续增加取消原因等字段预留空间。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelExportJobDto {}

/// 确认当前用户已经实际看到的导出完成或失败通知。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkExportNotificationsReadDto {
    #[schema(min_items = 1, max_items = 100, value_type = Vec<String>)]
    pub ids: Vec<String>,
}

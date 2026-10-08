use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow};

/// 岗位导出的稳定筛选条件。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PostExportReadFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

/// 岗位导出需要的最小只读数据行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostExportRow {
    /// 使用字符串保留 64 位主键的完整精度。
    pub id: String,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 岗位导出专用只读端口，业务 CRUD 不承担导出读取职责。
#[async_trait]
pub trait PostExportReadPort: Send + Sync {
    /// 按固定主键上界和递增游标读取一批岗位。
    async fn find_batch(
        &self,
        tenant_id: &str,
        filter: PostExportReadFilter<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<PostExportRow>>;
}

use std::sync::Arc;

use ryframe_kernel::{ActorContext, AppResult, ExportCursorWindow};

use crate::ports::export::{PostExportReadFilter, PostExportReadPort, PostExportRow};

/// 岗位导出扩展，只编排租户边界和稳定游标读取。
pub struct PostExportService {
    read: Arc<dyn PostExportReadPort>,
}

impl PostExportService {
    pub fn new(read: Arc<dyn PostExportReadPort>) -> Self {
        Self { read }
    }

    /// 读取不超过申请快照主键上界的一批岗位导出行。
    pub async fn find_batch(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostExportRow>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.read
            .find_batch(
                tenant_id,
                PostExportReadFilter { name, code, status },
                window,
            )
            .await
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::ports::export::{PostExportReadFilter, PostExportReadPort, PostExportRow};
use ryframe_kernel::{AppResult, ExportCursorWindow};

use crate::{
    ControlDatabaseCluster, PostExportFilter, PostExportRepository, ReadConsistency,
    generated::entities::post,
};

/// 构造岗位导出专用的强一致只读端口。
pub fn port(database: ControlDatabaseCluster) -> Arc<dyn PostExportReadPort> {
    Arc::new(DatabasePostExportRead { database })
}

struct DatabasePostExportRead {
    database: ControlDatabaseCluster,
}

#[async_trait]
impl PostExportReadPort for DatabasePostExportRead {
    async fn find_batch(
        &self,
        tenant_id: &str,
        filter: PostExportReadFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostExportRow>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let filter = PostExportFilter {
            name: filter.name,
            code: filter.code,
            status: filter.status,
        };
        Ok(PostExportRepository
            .find_batch(&database, tenant_id, &filter, window)
            .await?
            .into_iter()
            .map(to_row)
            .collect())
    }
}

fn to_row(model: post::Model) -> PostExportRow {
    PostExportRow {
        id: model.id.to_string(),
        name: model.name,
        code: model.code,
        sort: model.sort,
        status: model.status,
        remark: model.remark,
        created_at: model.created_at,
    }
}

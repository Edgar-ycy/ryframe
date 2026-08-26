use crate::DbResultExt;
use ryframe_kernel::{AppResult, ExportCursorWindow, ExportQuerySnapshot};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select,
};

use crate::generated::entities::post;

/// 岗位导出仓储使用的稳定筛选条件。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PostExportFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

/// 岗位导出专用只读仓储。
pub struct PostExportRepository;

impl PostExportRepository {
    /// 按申请时固定的主键上界读取下一批岗位。
    pub async fn find_batch<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &PostExportFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<post::Model>>
    where
        C: ConnectionTrait,
    {
        post_export_batch_query(tenant_id, filter, window)
            .all(db)
            .await
            .db()
    }

    /// 在导出申请的主库事务内统计匹配行并固定最大主键。
    pub async fn summarize<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &PostExportFilter<'_>,
    ) -> AppResult<ExportQuerySnapshot>
    where
        C: ConnectionTrait,
    {
        super::summarize_export_query(filtered_select(tenant_id, filter), post::Column::Id, db)
            .await
    }
}

fn filtered_select(tenant_id: &str, filter: &PostExportFilter<'_>) -> Select<post::Entity> {
    let mut select = post::Entity::find()
        .filter(post::Column::TenantId.eq(tenant_id))
        .filter(post::Column::DelFlag.eq(post::SOFT_DELETE_ACTIVE));
    if let Some(value) = filter.name.filter(|value| !value.is_empty()) {
        select = select.filter(post::Column::Name.like(format!("%{value}%")));
    }
    if let Some(value) = filter.code.filter(|value| !value.is_empty()) {
        select = select.filter(post::Column::Code.like(format!("%{value}%")));
    }
    if let Some(value) = filter.status.filter(|value| !value.is_empty()) {
        select = select.filter(post::Column::Status.eq(value));
    }
    select
}

/// 构造可确定检查的岗位导出批次查询。
#[doc(hidden)]
pub fn post_export_batch_query(
    tenant_id: &str,
    filter: &PostExportFilter<'_>,
    window: ExportCursorWindow,
) -> Select<post::Entity> {
    let mut select =
        filtered_select(tenant_id, filter).filter(post::Column::Id.lte(window.upper_id()));
    if let Some(after_id) = window.after_id() {
        select = select.filter(post::Column::Id.gt(after_id));
    }
    select.order_by_asc(post::Column::Id).limit(window.limit())
}

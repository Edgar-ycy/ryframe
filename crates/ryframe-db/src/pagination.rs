use ryframe_common::AppResult;
use ryframe_core::{PageQuery, PageResult};
use sea_orm::{DatabaseConnection, EntityTrait, FromQueryResult, PaginatorTrait, Select};
/// 执行分页查询
///
/// 使用方式：
/// ```ignore
/// let select = user::Entity::find().filter(user::Column::Status.eq(1));
/// let result = paginate(&db, select, &query).await?;
/// ```
pub async fn paginate<E>(
    db: &DatabaseConnection,
    select: Select<E>,
    query: &PageQuery,
) -> AppResult<PageResult<E::Model>>
where
    E: EntityTrait,
    E::Model: FromQueryResult + Send + Sync,
{
    let paginator = select.paginate(db, query.page_size);
    let total = paginator
        .num_items()
        .await
        .map_err(|e| ryframe_common::AppError::Database(format!("查询总数失败: {}", e)))?;

    let records = paginator
        .fetch_page(query.page.saturating_sub(1))
        .await
        .map_err(|e| ryframe_common::AppError::Database(format!("分页查询失败: {}", e)))?;

    Ok(PageResult::new(records, total, query))
}
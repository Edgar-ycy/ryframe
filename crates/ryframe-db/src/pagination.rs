use ryframe_common::AppResult;
use ryframe_core::{PageQuery, PageResult};
use sea_orm::{DatabaseConnection, EntityTrait, FromQueryResult, PaginatorTrait, Select};
/// 使用方式：
/// ```
/// use ryframe_core::{PageQuery, PageResult};
///
/// let query = PageQuery { page: 1, page_size: 10 };
/// assert_eq!(query.offset(), 0);
///
/// let result: PageResult<String> = PageResult::new(
///     vec!["item1".into(), "item2".into()],
///     2,
///     &query,
/// );
/// assert_eq!(result.total_pages(), 1);
/// assert_eq!(result.records.len(), 2);
/// ```
///
/// 实际分页查询需提供 `DatabaseConnection` 和 `Select<E>`：
/// ```text
/// let result = paginate(db, select, &query).await?;
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

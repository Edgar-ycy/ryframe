use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait, Select};
/// 使用方式：
/// ```text
/// use ryframe_config::PaginationConfig;
/// use ryframe_kernel::{PageResult, ValidatedPageQuery};
///
/// # fn main() -> ryframe_kernel::AppResult<()> {
/// let query = ValidatedPageQuery::new(1, 10, &PaginationConfig::default())?;
///
/// let result: PageResult<String> = PageResult::new(
///     vec!["item1".into(), "item2".into()],
///     2,
///     &query,
/// );
/// # Ok(())
/// # }
/// ```
///
/// 实际分页查询需提供 `DatabaseConnection` 和 `Select<E>`：
/// ```text
/// let result = paginate(db, select, &query).await?;
/// ```
pub async fn paginate<E, C>(
    db: &C,
    select: Select<E>,
    query: &ValidatedPageQuery,
) -> AppResult<PageResult<E::Model>>
where
    E: EntityTrait,
    E::Model: FromQueryResult + Send + Sync,
    C: ConnectionTrait,
{
    let paginator = select.paginate(db, query.page_size());
    let total = paginator
        .num_items()
        .await
        .map_err(|e| ryframe_kernel::AppError::Database(format!("查询总数失败: {}", e)))?;

    let records = paginator
        .fetch_page(query.page() - 1)
        .await
        .map_err(|e| ryframe_kernel::AppError::Database(format!("分页查询失败: {}", e)))?;

    Ok(PageResult::new(records, total, query))
}

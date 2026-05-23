//! 分页参数校验提取器
//!
//! 自动从 Query 参数中提取并校验分页参数，
//! 确保 page >= 1 且 page_size 在合理范围内。

use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use ryframe_core::PageQuery;
use serde::Deserialize;

/// 最大每页记录数
const MAX_PAGE_SIZE: u64 = 1000;

/// 原始分页参数（支持多种命名风格）
#[derive(Debug, Deserialize)]
struct RawPageParams {
    #[serde(default)]
    page: Option<u64>,
    #[serde(default, alias = "pageNum")]
    page_num: Option<u64>,
    #[serde(default, alias = "pageSize", alias = "size")]
    page_size: Option<u64>,
}

/// 经验证的分页查询参数
///
/// 自动从 URL query 参数中提取 `page` 和 `page_size`，
/// 并进行规范化（page >= 1, 1 <= page_size <= 500）。
///
/// 支持的参数名：
/// - `page` / `pageNum`：页码
/// - `page_size` / `pageSize` / `size`：每页条数
///
/// # 用法
/// ```ignore
/// async fn list(ValidatedPageQuery(query): ValidatedPageQuery) -> ... {
///     // query.page 和 query.page_size 已校验
/// }
/// ```
pub struct ValidatedPageQuery(pub PageQuery);

impl<S: Send + Sync> FromRequestParts<S> for ValidatedPageQuery {
    type Rejection = ryframe_common::AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(raw): Query<RawPageParams> =
            Query::from_request_parts(parts, state).await.map_err(|e| {
                ryframe_common::AppError::Validation(format!("分页参数解析失败: {}", e))
            })?;

        let page = raw.page.or(raw.page_num).unwrap_or(1);
        let page_size = raw.page_size.unwrap_or(10);

        let query = PageQuery { page, page_size }.normalize(MAX_PAGE_SIZE);

        Ok(ValidatedPageQuery(query))
    }
}

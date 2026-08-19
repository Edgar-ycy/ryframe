use crate::http::HttpResult;
use ryframe_adapters::ValidatedPageQuery;
use ryframe_config::PaginationConfig;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 数据保留运行记录分页参数。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct RetentionRunPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
}

impl RetentionRunPageQuery {
    pub fn into_page(self, policy: &PaginationConfig) -> HttpResult<ValidatedPageQuery> {
        Ok(ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            policy,
        )?)
    }
}

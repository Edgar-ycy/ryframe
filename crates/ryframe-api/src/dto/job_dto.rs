use ryframe_config::PaginationConfig;
use ryframe_core::PageQuery;
use ryframe_http::AppResult;
use ryframe_service::BackgroundJobListParams;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 后台任务分页查询参数。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct BackgroundJobPageQuery {
    /// 页码，从 1 开始；省略时采用运行时分页配置。
    #[param(minimum = 1)]
    pub page: Option<u64>,
    /// 每页记录数；上限由 `pagination.max_page_size` 决定。
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
    /// 按任务类型精确过滤。
    pub job_type: Option<String>,
    /// 按状态精确过滤：pending、running、succeeded 或 dead。
    pub status: Option<String>,
}

impl BackgroundJobPageQuery {
    pub fn into_service_params(
        self,
        policy: &PaginationConfig,
    ) -> AppResult<BackgroundJobListParams> {
        Ok(BackgroundJobListParams {
            page: PageQuery::from_optional(self.page, self.page_size, policy)?,
            job_type: self.job_type,
            status: self.status,
        })
    }
}

use crate::http::HttpResult;
use ryframe_adapters::ValidatedPageQuery;
use ryframe_config::PaginationConfig;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 用户导入任务分页查询。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct UserImportPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
    pub status: Option<String>,
}

impl UserImportPageQuery {
    pub fn into_page(self, policy: &PaginationConfig) -> HttpResult<ValidatedPageQuery> {
        Ok(ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            policy,
        )?)
    }
}

/// 用户导入异常行分页查询。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct UserImportRowPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
}

impl UserImportRowPageQuery {
    pub fn into_page(self, policy: &PaginationConfig) -> HttpResult<ValidatedPageQuery> {
        Ok(ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            policy,
        )?)
    }
}

/// OpenAPI 中的严格单文件上传表单。
#[derive(ToSchema)]
pub struct UserImportUploadForm {
    #[schema(value_type = String, format = Binary)]
    pub file: String,
}

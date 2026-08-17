use ryframe_config::PaginationConfig;
use ryframe_core::ValidatedPageQuery;
use ryframe_http::HttpResult;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 配置包与配置迁移分页查询。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct TenantConfigPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
}

impl TenantConfigPageQuery {
    pub fn into_page(self, policy: &PaginationConfig) -> HttpResult<ValidatedPageQuery> {
        Ok(ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            policy,
        )?)
    }
}

/// OpenAPI 中的严格单文件配置包上传表单。
#[derive(ToSchema)]
pub struct TenantConfigPackageUploadForm {
    #[schema(value_type = String, format = Binary)]
    pub file: String,
}

/// 从当前租户已有配置包创建一次迁移。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTenantConfigTransferDto {
    pub bundle_id: String,
}

/// 应用已经预览并由用户确认的配置迁移计划。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyTenantConfigTransferDto {
    pub plan_hash: String,
    pub target_configuration_version: i64,
    #[schema(pattern = r"^[0-9]+$")]
    pub target_authorization_epoch: String,
}

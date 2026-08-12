use serde::Deserialize;
use utoipa::ToSchema;

use ryframe_config::PaginationConfig;
use ryframe_core::ValidatedPageQuery;
use ryframe_http::HttpResult;
use ryframe_service::system::ConfigListParams;

crate::list_query!(pub ConfigListQuery, ConfigFilterQuery {
    name: String,
    key: String,
});

impl ConfigListQuery {
    pub fn into_service_params(self, policy: &PaginationConfig) -> HttpResult<ConfigListParams> {
        let (page, filter) = self.into_parts(policy)?;
        Ok(filter.into_service_params(page))
    }
}

impl ConfigFilterQuery {
    pub fn into_service_params(self, page: ValidatedPageQuery) -> ConfigListParams {
        ConfigListParams {
            page,
            name: self.name,
            key: self.key,
        }
    }
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateConfigDto {
    #[validate(length(min = 1, max = 100, message = "参数名称长度为1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "参数键名长度为1-100"))]
    pub key: String,
    #[validate(length(min = 1, max = 500, message = "参数键值长度为1-500"))]
    pub value: String,
    #[serde(default)]
    pub portable: bool,
    pub remark: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConfigDto {
    #[validate(length(min = 1, message = "配置值不能为空"))]
    pub value: String,
    #[serde(default)]
    pub portable: Option<bool>,
}

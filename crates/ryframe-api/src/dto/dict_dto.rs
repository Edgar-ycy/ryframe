use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDictTypeDto {
    #[validate(length(min = 1, message = "字典名称不能为空"))]
    pub name: String,
    #[validate(length(min = 1, message = "字典编码不能为空"))]
    pub code: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDictTypeDto {
    #[validate(length(min = 1, message = "字典名称不能为空"))]
    pub name: String,
    pub status: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDictDataDto {
    pub type_code: String,
    #[validate(length(min = 1, message = "字典标签不能为空"))]
    pub label: String,
    #[validate(length(min = 1, message = "字典值不能为空"))]
    pub value: String,
    pub sort: Option<i32>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDictDataDto {
    #[validate(length(min = 1, message = "字典标签不能为空"))]
    pub label: String,
    #[validate(length(min = 1, message = "字典值不能为空"))]
    pub value: String,
    pub sort: Option<i32>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DictOptionDto {
    pub label: String,
    pub value: String,
    pub css_class: Option<String>,
}

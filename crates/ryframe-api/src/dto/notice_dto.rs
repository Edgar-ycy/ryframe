use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateNoticeDto {
    #[validate(length(min = 1, message = "标题不能为空"))]
    pub title: String,
    #[validate(length(min = 1, message = "内容不能为空"))]
    pub content: String,
    pub notice_type: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNoticeDto {
    #[validate(length(min = 1, message = "标题不能为空"))]
    pub title: String,
    #[validate(length(min = 1, message = "内容不能为空"))]
    pub content: String,
    pub notice_type: Option<String>,
    pub status: String,
}

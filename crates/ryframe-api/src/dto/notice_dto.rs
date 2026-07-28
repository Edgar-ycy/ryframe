use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateNoticeDto {
    #[validate(length(min = 1, max = 200, message = "标题长度必须在 1 到 200 个字符之间"))]
    pub title: String,
    #[validate(length(
        min = 1,
        max = 10_000,
        message = "Markdown 内容长度必须在 1 到 10000 个字符之间"
    ))]
    pub content: String,
    pub notice_type: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNoticeDto {
    #[validate(length(min = 1, max = 200, message = "标题长度必须在 1 到 200 个字符之间"))]
    pub title: String,
    #[validate(length(
        min = 1,
        max = 10_000,
        message = "Markdown 内容长度必须在 1 到 10000 个字符之间"
    ))]
    pub content: String,
    pub notice_type: Option<String>,
    pub status: String,
}

use serde::Deserialize;

#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreateNoticeDto {
    #[validate(length(min = 1, message = "标题不能为空"))]
    pub title: String,
    #[validate(length(min = 1, message = "内容不能为空"))]
    pub content: String,
    pub notice_type: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct UpdateNoticeDto {
    #[validate(length(min = 1, message = "标题不能为空"))]
    pub title: String,
    #[validate(length(min = 1, message = "内容不能为空"))]
    pub content: String,
    pub notice_type: Option<String>,
    pub status: String,
}

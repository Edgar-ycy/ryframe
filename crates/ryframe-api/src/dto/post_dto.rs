use serde::Deserialize;

#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreatePostDto {
    #[validate(length(min = 1, message = "岗位名称不能为空"))]
    pub name: String,
    #[validate(length(min = 1, message = "岗位编码不能为空"))]
    pub code: String,
    pub sort: Option<i32>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct UpdatePostDto {
    #[validate(length(min = 1, message = "岗位名称不能为空"))]
    pub name: String,
    pub sort: Option<i32>,
    pub status: String,
}

use serde::Deserialize;

/// 创建 Todo 的请求 DTO
#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreateTodoDto {
    #[validate(length(min = 1, message = "标题不能为空"))]
    pub title: String,
}

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Agent 只读列表的固定分页参数；未知过滤条件由服务层审计后拒绝。
#[derive(Debug, Clone, Copy, Default, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AgentPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentCapabilityResponse {
    pub key: String,
    pub method: String,
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentUserResponse {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub dept_name: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentDepartmentResponse {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentPostResponse {
    pub id: String,
    pub code: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentDictionaryItemResponse {
    pub label: String,
    pub value: String,
    pub sort: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentDictionaryResponse {
    pub type_code: String,
    pub items: Vec<AgentDictionaryItemResponse>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub max_page_size: u64,
}

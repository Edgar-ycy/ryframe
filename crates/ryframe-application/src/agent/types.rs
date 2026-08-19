use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::AgentCapability;

/// Agent API 的一次调用输入。认证材料只保存在内存中，不实现 `Debug` 或序列化。
pub struct AgentRequest {
    pub capability: AgentCapability,
    pub authorization: Option<String>,
    pub delegation: Option<String>,
    pub page: u64,
    pub page_size: u64,
    pub type_code: Option<String>,
    pub request_id: String,
    pub client_ip: IpAddr,
    pub user_agent: Option<String>,
    /// 由 HTTP 边界完成本地化后的查询成功消息，用于计算最终响应的精确字节数。
    pub success_message: String,
    /// HTTP 边界开始处理请求的时间，用于审计真实耗时。
    pub started_at: DateTime<Utc>,
    /// HTTP 查询参数提取失败时携带稳定原因；服务仍先完成审计再返回校验错误。
    pub validation_error: Option<String>,
}

/// 与普通用户 JWT 完全隔离的 Agent 调用主体。
#[derive(Clone)]
pub struct AgentPrincipal {
    pub tenant_id: String,
    pub account_id: i64,
    pub credential_id: i64,
    pub delegation_id: Option<i64>,
    pub represented_user_id: Option<i64>,
    pub access_mode: AgentAccessMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentAccessMode {
    Direct,
    Delegated,
}

impl AgentAccessMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Delegated => "delegated",
        }
    }
}

/// 已完成查询与同事务审计的最终 Agent 响应。
pub struct AgentSuccess {
    /// 该字节串已经包含统一响应信封，HTTP 层必须原样返回。
    pub body: Vec<u8>,
    pub principal: AgentPrincipal,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentPage<T>
where
    T: Serialize,
{
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub max_page_size: u64,
}

impl<T> AgentPage<T>
where
    T: Serialize,
{
    pub fn new(items: Vec<T>, page: u64, page_size: u64, total: u64, max_page_size: u64) -> Self {
        Self {
            items,
            page,
            page_size,
            total,
            total_pages: total.div_ceil(page_size.max(1)),
            max_page_size,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentCapabilityVo {
    pub key: &'static str,
    pub method: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentUserVo {
    /// Snowflake ID 使用字符串，避免 JavaScript 精度损失。
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub dept_name: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentDepartmentVo {
    /// Snowflake ID 使用字符串，避免 JavaScript 精度损失。
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentPostVo {
    /// Snowflake ID 使用字符串，避免 JavaScript 精度损失。
    pub id: String,
    pub code: String,
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentDictionaryVo {
    pub type_code: String,
    pub items: Vec<AgentDictionaryItemVo>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub max_page_size: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentDictionaryItemVo {
    pub label: String,
    pub value: String,
    pub sort: i32,
}

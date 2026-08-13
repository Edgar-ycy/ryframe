use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Agent API 的成功和拒绝访问审计，不保存凭据、委托令牌或结果正文。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_service_access_audit")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub request_id: String,
    pub tenant_id: Option<String>,
    pub account_id: Option<i64>,
    pub credential_id: Option<i64>,
    pub delegation_id: Option<i64>,
    pub represented_user_id: Option<i64>,
    pub operation_id: String,
    pub capability_key: String,
    pub required_permission: String,
    pub access_mode: String,
    pub result: String,
    pub reason_code: String,
    pub http_status: i32,
    pub request_ip_digest: Option<Vec<u8>>,
    pub user_agent_digest: Option<Vec<u8>>,
    pub row_count: Option<i32>,
    pub response_bytes: Option<i64>,
    pub tenant_epoch: Option<i32>,
    pub account_authorization_version: Option<i32>,
    pub user_authorization_version: Option<i32>,
    pub delegation_version: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

impl Model {
    pub const ACCESS_MODE_DIRECT: &str = "direct";
    pub const ACCESS_MODE_DELEGATED: &str = "delegated";
    pub const RESULT_SUCCESS: &str = "success";
    pub const RESULT_DENIED: &str = "denied";
    pub const RESULT_ERROR: &str = "error";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

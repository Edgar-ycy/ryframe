use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// API Key 凭据元数据；数据库只保存 Secret 的 MAC，不保存明文。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_service_credential")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
    pub key_id: String,
    #[serde(skip_serializing)]
    pub secret_mac: Vec<u8>,
    pub pepper_version: i32,
    pub label: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_by: i64,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub idempotency_key_hash: Vec<u8>,
    #[serde(skip_serializing)]
    pub request_fingerprint: Vec<u8>,
}

impl Model {
    pub const STATUS_ACTIVE: &str = "active";
    pub const STATUS_REVOKED: &str = "revoked";

    pub fn is_usable_at(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_ACTIVE && self.revoked_at.is_none() && self.expires_at > now
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

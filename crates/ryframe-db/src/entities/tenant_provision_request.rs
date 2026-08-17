use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

/// 租户创建 Saga 的持久幂等请求；request_token 是原始幂等键与完整请求的 HMAC，
/// 不保存原始幂等键、管理员明文密码或可逆请求体。
#[derive(Clone, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "sys_tenant_provision_request")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: String,
    pub request_token: String,
    pub admin_password_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TenantProvisionRequest")
            .field("tenant_id", &self.tenant_id)
            .field("request_token", &"<redacted>")
            .field("admin_password_hash", &"<redacted>")
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

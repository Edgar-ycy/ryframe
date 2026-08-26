use ryframe_kernel::AppResult;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect,
    sea_query::LockType,
};

use super::TenantProvisioningRepository;
use crate::{
    DbResultExt,
    entities::{tenant::provision_request as tenant_provision_request, user},
};

impl TenantProvisioningRepository {
    pub async fn lock_provision_request_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<Option<tenant_provision_request::Model>> {
        tenant_provision_request::Entity::find_by_id(tenant_id.to_owned())
            .lock(LockType::Update)
            .one(transaction)
            .await
            .db()
    }

    pub async fn admin_username_exists(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        username: &str,
    ) -> AppResult<bool> {
        user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await
            .map(|user| user.is_some())
            .db()
    }

    pub async fn find_user_by_username<C>(
        &self,
        db: &C,
        tenant_id: &str,
        username: &str,
    ) -> AppResult<Option<user::Model>>
    where
        C: sea_orm::ConnectionTrait,
    {
        user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await
            .db()
    }
}

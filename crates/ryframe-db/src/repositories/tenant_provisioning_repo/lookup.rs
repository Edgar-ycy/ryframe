use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, sea_query::LockType};

use super::*;

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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))
    }
}

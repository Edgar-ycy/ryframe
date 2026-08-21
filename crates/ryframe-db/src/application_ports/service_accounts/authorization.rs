use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    ControlDatabaseCluster, PermissionRepository, ReadConsistency, Repository, RoleRepository,
    ServiceAccountRepository,
    entities::{permission, role, role_permission, service_account, service_account_role},
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};

use ryframe_application::{
    PersistenceFuture,
    ports::service_accounts::{
        ServiceAccountAuthorizationReadPort, ServiceAccountPermissionSnapshot,
        ServiceDelegationTargetRecord, ServiceDelegationTargetSet,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ServiceAccountAuthorizationReadPort> {
    Arc::new(DatabaseServiceAccountAuthorizationPersistence { database })
}

struct DatabaseServiceAccountAuthorizationPersistence {
    database: ControlDatabaseCluster,
}

impl ServiceAccountAuthorizationReadPort for DatabaseServiceAccountAuthorizationPersistence {
    fn permission_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountPermissionSnapshot>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let account = ServiceAccountRepository
                .find_by_id(&database, tenant_id, account_id)
                .await?;
            if !account.is_some_and(|account| account.is_enabled()) {
                return Ok(None);
            }
            let user_roles = RoleRepository
                .find_user_roles(&database, tenant_id, user_id)
                .await?;
            let user_role_ids = user_roles
                .into_iter()
                .map(|role| role.id)
                .collect::<Vec<_>>();
            let user_permissions = permission_codes(&database, tenant_id, &user_role_ids).await?;

            let account_role_ids = ServiceAccountRepository
                .role_ids(&database, tenant_id, account_id)
                .await?;
            let enabled_account_role_ids =
                enabled_role_ids(&database, tenant_id, account_role_ids.iter().copied()).await?;
            let account_permissions =
                permission_codes(&database, tenant_id, &enabled_account_role_ids).await?;
            Ok(Some(ServiceAccountPermissionSnapshot {
                user_permissions,
                account_permissions,
            }))
        })
    }

    fn delegation_targets<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        limit: u64,
    ) -> PersistenceFuture<'a, ServiceDelegationTargetSet> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let user_roles = RoleRepository
                .find_user_roles(&database, tenant_id, user_id)
                .await?;
            let user_role_ids = user_roles
                .into_iter()
                .map(|role| role.id)
                .collect::<Vec<_>>();
            let user_permissions = permission_codes(&database, tenant_id, &user_role_ids).await?;

            let account_query = service_account::Entity::find()
                .filter(service_account::Column::TenantId.eq(tenant_id))
                .filter(service_account::Column::Status.eq(service_account::Model::STATUS_NORMAL))
                .filter(
                    service_account::Column::DelFlag.eq(service_account::Model::DEL_FLAG_NORMAL),
                );
            if account_query
                .clone()
                .count(&database)
                .await
                .map_err(database_error)?
                > limit
            {
                return Err(AppError::Validation(format!(
                    "当前租户可用服务账号超过 {limit} 个，请由管理员收敛账号数量"
                )));
            }
            let accounts = account_query
                .order_by_asc(service_account::Column::Code)
                .all(&database)
                .await
                .map_err(database_error)?;
            let account_ids = accounts
                .iter()
                .map(|account| account.id)
                .collect::<Vec<_>>();
            let account_permissions =
                account_permission_codes(&database, tenant_id, &account_ids).await?;
            Ok(ServiceDelegationTargetSet {
                user_permissions,
                accounts: accounts
                    .into_iter()
                    .map(|account| ServiceDelegationTargetRecord {
                        account_id: account.id,
                        code: account.code,
                        name: account.name,
                        permission_codes: account_permissions
                            .get(&account.id)
                            .cloned()
                            .unwrap_or_default(),
                    })
                    .collect(),
            })
        })
    }
}

async fn permission_codes(
    database: &DatabaseConnection,
    tenant_id: &str,
    role_ids: &[i64],
) -> AppResult<HashSet<String>> {
    Ok(PermissionRepository
        .find_role_perms(database, tenant_id, role_ids)
        .await?
        .into_iter()
        .map(|permission| permission.code)
        .collect())
}

async fn enabled_role_ids<I>(
    database: &DatabaseConnection,
    tenant_id: &str,
    role_ids: I,
) -> AppResult<Vec<i64>>
where
    I: IntoIterator<Item = i64>,
{
    let role_ids = role_ids.into_iter().collect::<Vec<_>>();
    if role_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::Id.is_in(role_ids))
        .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .all(database)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|role| role.id)
        .collect())
}

async fn account_permission_codes(
    database: &DatabaseConnection,
    tenant_id: &str,
    account_ids: &[i64],
) -> AppResult<HashMap<i64, HashSet<String>>> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let account_roles = service_account_role::Entity::find()
        .filter(service_account_role::Column::TenantId.eq(tenant_id))
        .filter(service_account_role::Column::AccountId.is_in(account_ids.iter().copied()))
        .all(database)
        .await
        .map_err(database_error)?;
    let enabled_role_ids = enabled_role_ids(
        database,
        tenant_id,
        account_roles.iter().map(|relation| relation.role_id),
    )
    .await?;
    if enabled_role_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let role_permissions = role_permission::Entity::find()
        .filter(role_permission::Column::TenantId.eq(tenant_id))
        .filter(role_permission::Column::RoleId.is_in(enabled_role_ids.iter().copied()))
        .all(database)
        .await
        .map_err(database_error)?;
    let permission_ids = role_permissions
        .iter()
        .map(|relation| relation.perm_id)
        .collect::<HashSet<_>>();
    let permission_codes = if permission_ids.is_empty() {
        HashMap::new()
    } else {
        permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Id.is_in(permission_ids))
            .filter(permission::Column::Status.eq("1"))
            .all(database)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|permission| (permission.id, permission.code))
            .collect::<HashMap<_, _>>()
    };
    let role_to_permissions = role_permissions.into_iter().fold(
        HashMap::<i64, HashSet<String>>::new(),
        |mut mapping, relation| {
            if let Some(code) = permission_codes.get(&relation.perm_id) {
                mapping
                    .entry(relation.role_id)
                    .or_default()
                    .insert(code.clone());
            }
            mapping
        },
    );
    let mut result = HashMap::<i64, HashSet<String>>::new();
    for relation in account_roles {
        if let Some(codes) = role_to_permissions.get(&relation.role_id) {
            result
                .entry(relation.account_id)
                .or_default()
                .extend(codes.iter().cloned());
        }
    }
    Ok(result)
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

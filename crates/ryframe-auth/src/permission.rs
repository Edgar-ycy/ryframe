use ryframe_common::{AppError, AppResult};
use ryframe_core::{
    RedisClient,
    cache::{get_user_permission_cache, set_user_permission_cache},
};
use ryframe_db::{PermissionRepository, RoleRepository};
use sea_orm::DatabaseConnection;

use crate::rbac;

/// Roles and permissions resolved from the current database state.
#[derive(Debug, Clone)]
pub struct PermissionContext {
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    pub is_super_admin: bool,
}

/// Resolve all enabled roles and API permission codes for one user.
///
/// Redis is only an optimization. A cache failure falls back to the database,
/// while database errors are returned to the caller.
pub async fn resolve_user_permission_context(
    db: &DatabaseConnection,
    redis: Option<&RedisClient>,
    tenant_id: &str,
    user_id: i64,
) -> AppResult<PermissionContext> {
    let roles = RoleRepository.find_user_roles(db, user_id).await?;
    let role_codes: Vec<String> = roles.iter().map(|role| role.code.clone()).collect();
    let is_super_admin = roles.iter().any(|role| role.is_super == 1);

    let permissions = if is_super_admin {
        vec!["*:*:*".to_string()]
    } else if let Some(redis) = redis {
        match get_user_permission_cache(redis, tenant_id, user_id).await {
            Ok(Some(cached)) => cached,
            Ok(None) | Err(_) => {
                let loaded = load_user_permission_codes(db, &roles).await?;
                if let Err(error) =
                    set_user_permission_cache(redis, tenant_id, user_id, &loaded).await
                {
                    tracing::warn!(
                        tenant_id,
                        user_id,
                        %error,
                        "failed to cache user permissions"
                    );
                }
                loaded
            }
        }
    } else {
        load_user_permission_codes(db, &roles).await?
    };

    Ok(PermissionContext {
        roles: role_codes,
        permissions,
        is_super_admin,
    })
}

async fn load_user_permission_codes(
    db: &DatabaseConnection,
    roles: &[ryframe_db::entities::role::Model],
) -> AppResult<Vec<String>> {
    let role_ids: Vec<i64> = roles.iter().map(|role| role.id).collect();
    let mut codes: Vec<String> = PermissionRepository
        .find_role_perms(db, &role_ids)
        .await?
        .into_iter()
        .map(|permission| permission.code)
        .collect();
    codes.sort();
    codes.dedup();
    Ok(codes)
}

pub fn check_permission_context(context: &PermissionContext, required: &str) -> AppResult<()> {
    if context.is_super_admin || rbac::has_permission(&context.permissions, required) {
        return Ok(());
    }
    Err(AppError::Authorization(format!(
        "权限不足，需要权限码: {required}"
    )))
}

use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;
use time::OffsetDateTime;

use crate::{
    ControlDatabaseCluster, DbResultExt, DeptRepository, PermissionRepository, Repository,
    RoleRepository, TenantRepository, UserRepository,
};

use ryframe_application::ports::auth::{
    IdentityAuthorizationReadPort, IdentityRoleRecord, IdentityTenantRecord, IdentityUserRecord,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn IdentityAuthorizationReadPort> {
    Arc::new(DatabaseIdentityAuthorization { database })
}

struct DatabaseIdentityAuthorization {
    database: ControlDatabaseCluster,
}

#[async_trait::async_trait]
impl IdentityAuthorizationReadPort for DatabaseIdentityAuthorization {
    async fn tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<IdentityTenantRecord>> {
        TenantRepository
            .find_by_tenant_id(self.database.write(), tenant_id)
            .await?
            .map(to_tenant)
            .transpose()
    }

    async fn user_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<IdentityUserRecord>> {
        Ok(UserRepository
            .find_by_id(self.database.write(), tenant_id, user_id)
            .await?
            .map(to_user))
    }

    async fn user_by_username<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> ryframe_kernel::AppResult<Option<IdentityUserRecord>> {
        Ok(UserRepository
            .find_by_username(self.database.write(), tenant_id, username)
            .await?
            .map(to_user))
    }

    async fn roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<IdentityRoleRecord>> {
        Ok(RoleRepository
            .find_user_roles(self.database.write(), tenant_id, user_id)
            .await?
            .into_iter()
            .map(|role| IdentityRoleRecord {
                id: role.id,
                code: role.code,
                is_super: role.is_super == 1,
                data_scope: role.data_scope,
            })
            .collect())
    }

    async fn permission_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<String>> {
        Ok(PermissionRepository
            .find_role_perms(self.database.write(), tenant_id, role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect())
    }

    async fn department_name<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> ryframe_kernel::AppResult<Option<String>> {
        Ok(DeptRepository
            .find_by_id(self.database.write(), tenant_id, dept_id)
            .await?
            .map(|department| department.name))
    }

    async fn department_ancestors<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> ryframe_kernel::AppResult<Option<String>> {
        Ok(DeptRepository
            .find_by_id(self.database.write(), tenant_id, dept_id)
            .await?
            .map(|department| department.ancestors))
    }

    async fn role_department_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<i64>> {
        RoleRepository
            .find_roles_dept_ids(self.database.write(), tenant_id, role_ids)
            .await
    }

    async fn child_department_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<i64>> {
        DeptRepository
            .find_child_dept_ids(self.database.write(), tenant_id, dept_id)
            .await
    }
}

fn to_tenant(tenant: crate::entities::tenant::Model) -> AppResult<IdentityTenantRecord> {
    Ok(IdentityTenantRecord {
        tenant_id: tenant.tenant_id,
        name: tenant.name,
        status: tenant.status,
        expire_at: tenant.expire_at.map(tenant_expiry).transpose()?,
        max_requests_per_min: tenant.max_requests_per_min,
        session_version: tenant.session_version,
        authorization_epoch: tenant.authorization_epoch,
        data_scope_version: tenant.data_scope_version,
    })
}

fn tenant_expiry(value: DateTime<Utc>) -> AppResult<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(value.timestamp())
        .and_then(|instant| instant.replace_nanosecond(value.timestamp_subsec_nanos()))
        .db_context("读取身份租户到期时间")
}

fn to_user(user: crate::entities::user::Model) -> IdentityUserRecord {
    IdentityUserRecord {
        id: user.id,
        tenant_id: user.tenant_id,
        username: user.username,
        password_hash: user.password_hash,
        password_change_required: user.password_change_required,
        nickname: user.nickname,
        email: user.email,
        phone: user.phone,
        avatar: user.avatar,
        preferred_locale: user.preferred_locale,
        status: user.status,
        authorization_version: user.authorization_version,
        dept_id: user.dept_id,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use ryframe_kernel::AppError;

    use super::*;

    #[test]
    fn tenant_expiry_retains_mysql_calendar_and_subsecond_precision() {
        for stored in [
            Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).unwrap(),
            DateTime::from_timestamp(-1, 123_456_000).unwrap(),
            Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59).unwrap(),
        ] {
            let expiry = tenant_expiry(stored).unwrap();
            assert_eq!(expiry.unix_timestamp(), stored.timestamp());
            assert_eq!(expiry.nanosecond(), stored.timestamp_subsec_nanos());
            assert_eq!(expiry.offset(), time::UtcOffset::UTC);
        }
        let unrepresentable = DateTime::from_timestamp(-1, 1_123_456_000).unwrap();
        assert!(matches!(
            tenant_expiry(unrepresentable),
            Err(AppError::Database(_))
        ));
    }
}

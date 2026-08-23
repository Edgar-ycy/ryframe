use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, DeptRepository, MenuRepository, PermissionRepository, RoleRepository,
    UserRepository,
};

use ryframe_application::ports::authorization::{
    AuthorizationDiagnosticReadPort, DiagnosticDepartmentRecord, DiagnosticMenuRecord,
    DiagnosticPermissionRecord, DiagnosticRoleRecord,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn AuthorizationDiagnosticReadPort> {
    Arc::new(DatabaseAuthorizationDiagnosticPersistence { database })
}

struct DatabaseAuthorizationDiagnosticPersistence {
    database: ControlDatabaseCluster,
}

#[async_trait::async_trait]
impl AuthorizationDiagnosticReadPort for DatabaseAuthorizationDiagnosticPersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn user_tenant_id(&self, user_id: i64) -> ryframe_kernel::AppResult<Option<String>> {
        UserRepository
            .find_tenant_id_by_id(self.database.write(), user_id)
            .await
    }

    async fn assigned_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticRoleRecord>> {
        Ok(RoleRepository
            .find_user_roles_all_status(self.database.write(), tenant_id, user_id)
            .await?
            .into_iter()
            .map(|role| DiagnosticRoleRecord {
                id: role.id,
                name: role.name,
                code: role.code,
                status: role.status,
                data_scope: role.data_scope,
                is_super: role.is_super == 1,
            })
            .collect())
    }

    async fn permissions<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticPermissionRecord>> {
        Ok(PermissionRepository
            .find_all(self.database.write(), tenant_id)
            .await?
            .into_iter()
            .map(to_permission)
            .collect())
    }

    async fn role_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticPermissionRecord>> {
        Ok(PermissionRepository
            .find_role_perms(self.database.write(), tenant_id, &[role_id])
            .await?
            .into_iter()
            .map(to_permission)
            .collect())
    }

    async fn menus<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticMenuRecord>> {
        Ok(MenuRepository
            .find_all_for_diagnostics(self.database.write(), tenant_id)
            .await?
            .into_iter()
            .map(|menu| DiagnosticMenuRecord {
                id: menu.id,
                parent_id: menu.parent_id,
                name: menu.name,
                route_key: menu.route_key,
                perm_id: menu.perm_id,
                menu_type: menu.menu_type,
                status: menu.status,
                visible: menu.visible,
            })
            .collect())
    }

    async fn accessible_menu_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> ryframe_kernel::AppResult<Vec<i64>> {
        Ok(MenuRepository
            .find_by_permission_codes(self.database.write(), tenant_id, permission_codes)
            .await?
            .into_iter()
            .map(|menu| menu.id)
            .collect())
    }

    async fn departments<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticDepartmentRecord>> {
        Ok(DeptRepository
            .find_filtered_by_ids(self.database.write(), tenant_id, None, None, ids)
            .await?
            .into_iter()
            .map(|department| DiagnosticDepartmentRecord {
                id: department.id,
                name: department.name,
            })
            .collect())
    }
}

fn to_permission(permission: crate::entities::permission::Model) -> DiagnosticPermissionRecord {
    DiagnosticPermissionRecord {
        id: permission.id,
        name: permission.name,
        code: permission.code,
        status: permission.status,
    }
}

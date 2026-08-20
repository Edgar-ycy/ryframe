use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, DataRetentionRepository, DeptRepository, MenuRepository,
    PermissionRepository, RoleRepository, UserRepository,
};

use crate::{
    AuthorizationDiagnosticReadPort, DiagnosticDepartmentRecord, DiagnosticMenuRecord,
    DiagnosticPermissionRecord, DiagnosticRoleRecord, PersistenceFuture,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn AuthorizationDiagnosticReadPort> {
    Arc::new(LegacyAuthorizationDiagnosticPersistence { database })
}

struct LegacyAuthorizationDiagnosticPersistence {
    database: ControlDatabaseCluster,
}

impl AuthorizationDiagnosticReadPort for LegacyAuthorizationDiagnosticPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            DataRetentionRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn user_tenant_id(&self, user_id: i64) -> PersistenceFuture<'_, Option<String>> {
        Box::pin(async move {
            UserRepository
                .find_tenant_id_by_id(self.database.write(), user_id)
                .await
        })
    }

    fn assigned_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<DiagnosticRoleRecord>> {
        Box::pin(async move {
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
        })
    }

    fn permissions<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<DiagnosticPermissionRecord>> {
        Box::pin(async move {
            Ok(PermissionRepository
                .find_all(self.database.write(), tenant_id)
                .await?
                .into_iter()
                .map(to_permission)
                .collect())
        })
    }

    fn role_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<DiagnosticPermissionRecord>> {
        Box::pin(async move {
            Ok(PermissionRepository
                .find_role_perms(self.database.write(), tenant_id, &[role_id])
                .await?
                .into_iter()
                .map(to_permission)
                .collect())
        })
    }

    fn menus<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<DiagnosticMenuRecord>> {
        Box::pin(async move {
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
        })
    }

    fn accessible_menu_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<i64>> {
        Box::pin(async move {
            Ok(MenuRepository
                .find_by_permission_codes(self.database.write(), tenant_id, permission_codes)
                .await?
                .into_iter()
                .map(|menu| menu.id)
                .collect())
        })
    }

    fn departments<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<DiagnosticDepartmentRecord>> {
        Box::pin(async move {
            Ok(DeptRepository
                .find_filtered_by_ids(self.database.write(), tenant_id, None, None, ids)
                .await?
                .into_iter()
                .map(|department| DiagnosticDepartmentRecord {
                    id: department.id,
                    name: department.name,
                })
                .collect())
        })
    }
}

fn to_permission(
    permission: ryframe_db::entities::permission::Model,
) -> DiagnosticPermissionRecord {
    DiagnosticPermissionRecord {
        id: permission.id,
        name: permission.name,
        code: permission.code,
        status: permission.status,
    }
}

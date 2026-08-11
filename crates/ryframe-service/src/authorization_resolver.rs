use ryframe_core::Repository;
use ryframe_db::{
    DeptRepository, PermissionRepository, RoleRepository,
    entities::{role, user},
};
use ryframe_kernel::{AppResult, DataScope, DataScopeContext};
use sea_orm::DatabaseConnection;

/// 从主库计算得到的最终授权结果。
///
/// 登录、请求主体刷新、长时间后台任务和权限诊断必须复用该结果，避免不同入口对
/// 超级角色、停用角色或数据范围采用不同规则。
pub(crate) struct ResolvedAuthorization {
    pub roles: Vec<role::Model>,
    pub permission_codes: Vec<String>,
    pub data_scope: DataScopeContext,
}

pub(crate) struct AuthorizationResolver {
    role_repo: RoleRepository,
    permission_repo: PermissionRepository,
    dept_repo: DeptRepository,
}

impl AuthorizationResolver {
    pub fn new() -> Self {
        Self {
            role_repo: RoleRepository,
            permission_repo: PermissionRepository,
            dept_repo: DeptRepository,
        }
    }

    pub async fn resolve(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user: &user::Model,
    ) -> AppResult<ResolvedAuthorization> {
        let mut roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user.id)
            .await?;
        roles.sort_unstable_by_key(|role| role.id);
        let is_super_admin = roles.iter().any(|role| role.is_super == 1);
        let permission_codes = if is_super_admin {
            vec!["*:*:*".to_owned()]
        } else {
            let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
            let mut codes = self
                .permission_repo
                .find_role_perms(db, tenant_id, &role_ids)
                .await?
                .into_iter()
                .map(|permission| permission.code)
                .collect::<Vec<_>>();
            codes.sort_unstable();
            codes.dedup();
            codes
        };
        let data_scope = self
            .resolve_data_scope(db, tenant_id, user.id, user.dept_id, &roles)
            .await?;
        Ok(ResolvedAuthorization {
            roles,
            permission_codes,
            data_scope,
        })
    }

    async fn resolve_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        dept_id: Option<i64>,
        roles: &[role::Model],
    ) -> AppResult<DataScopeContext> {
        if roles.iter().any(|role| role.is_super == 1) {
            return Ok(DataScopeContext::super_admin(user_id));
        }

        let ancestors = match dept_id {
            Some(dept_id) => self
                .dept_repo
                .find_by_id(db, tenant_id, dept_id)
                .await?
                .map(|dept| dept.ancestors),
            None => None,
        };
        let custom_role_ids = roles
            .iter()
            .filter(|role| role.data_scope == role::Model::DATA_SCOPE_CUSTOM)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let custom_dept_ids = self
            .role_repo
            .find_roles_dept_ids(db, tenant_id, &custom_role_ids)
            .await?;
        let mut scopes = Vec::with_capacity(roles.len());

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match dept_id {
                    Some(dept_id) => {
                        self.dept_repo
                            .find_child_dept_ids(db, tenant_id, dept_id)
                            .await?
                    }
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
            });
        }

        if scopes.is_empty() {
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id,
                dept_id,
                ancestors,
                custom_dept_ids: Vec::new(),
                include_self: true,
            });
        }
        Ok(DataScopeContext::merge(scopes))
    }
}

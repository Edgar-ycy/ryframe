use std::{collections::HashMap, sync::Arc};

use crate::{
    ControlDatabaseCluster, DeptRepository, ReadConsistency, Repository, RoleRepository,
    UserFilter, UserRepository,
};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::DatabaseConnection;

use ryframe_application::ports::system::DeptRecord;
use ryframe_application::ports::users::{
    UserQueryDetailRecord, UserQueryFilter, UserQueryReadPort, UserQueryRecord, UserQueryRoleRecord,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn UserQueryReadPort> {
    Arc::new(DatabaseUserQueryPersistence { database })
}

struct DatabaseUserQueryPersistence {
    database: ControlDatabaseCluster,
}

#[async_trait::async_trait]
impl UserQueryReadPort for DatabaseUserQueryPersistence {
    async fn export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> ryframe_kernel::AppResult<Vec<UserQueryRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let records = UserRepository
            .find_for_export_after_id(&database, tenant_id, &to_filter(filter), scope, window)
            .await?
            .into_iter()
            .map(to_user)
            .collect();
        fill_department_names(&database, tenant_id, records).await
    }

    async fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        query: ValidatedPageQuery,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<PageResult<UserQueryRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let page = UserRepository
            .find_by_page_filtered_with_data_scope(
                &database,
                tenant_id,
                &query,
                &to_filter(filter),
                scope,
            )
            .await?;
        let records = page.records.into_iter().map(to_user).collect();
        let records = fill_department_names(&database, tenant_id, records).await?;
        Ok(PageResult::new(records, page.total, &query))
    }

    async fn options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        scope: &'a DataScopeContext,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<UserQueryRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        Ok(UserRepository
            .find_options_with_data_scope(&database, tenant_id, query, scope, limit)
            .await?
            .into_iter()
            .map(to_user)
            .collect())
    }

    async fn detail<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<Option<UserQueryDetailRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let Some(user) = UserRepository
            .find_by_id_with_data_scope(&database, tenant_id, user_id, scope)
            .await?
        else {
            return Ok(None);
        };
        let department = match user.dept_id {
            Some(department_id) => DeptRepository
                .find_by_id(&database, tenant_id, department_id)
                .await?
                .map(to_department),
            None => None,
        };
        let mut user = to_user(user);
        user.dept_name = department
            .as_ref()
            .map(|department| department.name.clone());
        let roles = RoleRepository
            .find_user_roles(&database, tenant_id, user_id)
            .await?
            .into_iter()
            .map(|role| UserQueryRoleRecord {
                id: role.id,
                name: role.name,
                code: role.code,
                is_super: role.is_super,
            })
            .collect();
        Ok(Some(UserQueryDetailRecord {
            user,
            department,
            roles,
        }))
    }

    async fn is_accessible<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<bool> {
        UserRepository
            .find_by_id_with_data_scope(self.database.write(), tenant_id, user_id, scope)
            .await
            .map(|user| user.is_some())
    }

    async fn is_super_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<bool> {
        RoleRepository
            .find_user_roles_all_status(self.database.write(), tenant_id, user_id)
            .await
            .map(|roles| roles.into_iter().any(|role| role.is_super == 1))
    }
}

fn to_filter(filter: UserQueryFilter<'_>) -> UserFilter<'_> {
    UserFilter {
        username: filter.username,
        phone: filter.phone,
        status: filter.status,
        dept_id: filter.dept_id,
    }
}

fn to_user(user: crate::entities::user::Model) -> UserQueryRecord {
    UserQueryRecord {
        id: user.id,
        username: user.username,
        nickname: user.nickname,
        email: user.email,
        phone: user.phone,
        avatar: user.avatar,
        status: user.status,
        dept_id: user.dept_id,
        dept_name: None,
        remark: user.remark,
        created_at: user.created_at,
    }
}

fn to_department(department: crate::entities::dept::Model) -> DeptRecord {
    DeptRecord {
        id: department.id,
        name: department.name,
        parent_id: department.parent_id,
        ancestors: department.ancestors,
        sort: department.sort,
        status: department.status,
        remark: department.remark,
        created_at: department.created_at,
        updated_at: department.updated_at,
    }
}

async fn fill_department_names(
    database: &DatabaseConnection,
    tenant_id: &str,
    mut users: Vec<UserQueryRecord>,
) -> ryframe_kernel::AppResult<Vec<UserQueryRecord>> {
    let mut department_ids = users
        .iter()
        .filter_map(|user| user.dept_id)
        .collect::<Vec<_>>();
    department_ids.sort_unstable();
    department_ids.dedup();
    let department_names = DeptRepository
        .find_filtered_by_ids(database, tenant_id, None, None, &department_ids)
        .await?
        .into_iter()
        .map(|department| (department.id, department.name))
        .collect::<HashMap<_, _>>();
    for user in &mut users {
        user.dept_name = user
            .dept_id
            .and_then(|department_id| department_names.get(&department_id).cloned());
    }
    Ok(users)
}

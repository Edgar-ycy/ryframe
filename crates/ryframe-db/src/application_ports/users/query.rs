use std::{collections::HashMap, sync::Arc};

use crate::{
    ControlDatabaseCluster, DeptRepository, ReadConsistency, RoleRepository, UserFilter,
    UserRepository,
};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::DatabaseConnection;

use ryframe_application::{
    PersistenceFuture,
    ports::users::{
        UserQueryDetailRecord, UserQueryFilter, UserQueryReadPort, UserQueryRecord,
        UserQueryRoleRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn UserQueryReadPort> {
    Arc::new(DatabaseUserQueryPersistence { database })
}

struct DatabaseUserQueryPersistence {
    database: ControlDatabaseCluster,
}

impl UserQueryReadPort for DatabaseUserQueryPersistence {
    fn export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<UserQueryRecord>> {
        Box::pin(async move {
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
        })
    }

    fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        query: ValidatedPageQuery,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<UserQueryRecord>> {
        Box::pin(async move {
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
        })
    }

    fn options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        scope: &'a DataScopeContext,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<UserQueryRecord>> {
        Box::pin(async move {
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
        })
    }

    fn detail<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, Option<UserQueryDetailRecord>> {
        Box::pin(async move {
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
            let mut users =
                fill_department_names(&database, tenant_id, vec![to_user(user)]).await?;
            let user = users
                .pop()
                .ok_or_else(|| ryframe_kernel::AppError::Internal("用户查询结果丢失".into()))?;
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
            Ok(Some(UserQueryDetailRecord { user, roles }))
        })
    }

    fn is_accessible<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            UserRepository
                .find_by_id_with_data_scope(self.database.write(), tenant_id, user_id, scope)
                .await
                .map(|user| user.is_some())
        })
    }

    fn is_super_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            RoleRepository
                .find_user_roles_all_status(self.database.write(), tenant_id, user_id)
                .await
                .map(|roles| roles.into_iter().any(|role| role.is_super == 1))
        })
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

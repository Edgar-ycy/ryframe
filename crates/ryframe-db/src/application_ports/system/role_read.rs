use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, PermissionRepository, ReadConsistency, Repository,
    RoleFilter as DatabaseRoleFilter, RoleRepository, entities::role,
};
use async_trait::async_trait;
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

use ryframe_application::ports::system::{RoleFilter, RoleReadPort, RoleRecord};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn RoleReadPort> {
    Arc::new(DatabaseRoleRead { database })
}

struct DatabaseRoleRead {
    database: ControlDatabaseCluster,
}

#[async_trait]
impl RoleReadPort for DatabaseRoleRead {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<RoleRecord>> {
        Ok(RoleRepository
            .find_by_id(self.database.write(), tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: RoleFilter<'_>,
    ) -> AppResult<PageResult<RoleRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        let result = RoleRepository
            .find_by_page_filtered(
                &database,
                tenant_id,
                page,
                filter.name,
                filter.code,
                filter.status,
            )
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find_options(
        &self,
        tenant_id: &str,
        query: Option<&str>,
        include_super: bool,
        limit: u64,
    ) -> AppResult<Vec<RoleRecord>> {
        RoleRepository
            .find_options(
                self.database.write(),
                tenant_id,
                query,
                include_super,
                limit,
            )
            .await
            .map(|roles| roles.into_iter().map(to_record).collect())
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: RoleFilter<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<RoleRecord>> {
        RoleRepository
            .find_for_export_after_id(
                self.database.write(),
                tenant_id,
                &DatabaseRoleFilter {
                    name: filter.name,
                    code: filter.code,
                    status: filter.status,
                },
                window,
            )
            .await
            .map(|roles| roles.into_iter().map(to_record).collect())
    }

    async fn find_super_role(&self, tenant_id: &str) -> AppResult<Option<RoleRecord>> {
        Ok(RoleRepository
            .find_super_role(self.database.write(), tenant_id)
            .await?
            .map(to_record))
    }

    async fn find_role_dept_ids(&self, tenant_id: &str, role_id: i64) -> AppResult<Vec<i64>> {
        RoleRepository
            .find_role_dept_ids(self.database.write(), tenant_id, role_id)
            .await
    }

    async fn find_permission_codes(
        &self,
        tenant_id: &str,
        role_id: i64,
    ) -> AppResult<Option<Vec<String>>> {
        let database = self.database.write();
        if RoleRepository
            .find_by_id(database, tenant_id, role_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let mut codes = PermissionRepository
            .find_role_perms(database, tenant_id, &[role_id])
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        codes.sort();
        codes.dedup();
        Ok(Some(codes))
    }
}

fn to_record(model: role::Model) -> RoleRecord {
    RoleRecord {
        id: model.id,
        name: model.name,
        code: model.code,
        is_super: model.is_super,
        data_scope: model.data_scope,
        status: model.status,
        sort: model.sort,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

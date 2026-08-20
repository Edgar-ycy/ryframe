use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, PermissionRepository, ReadConsistency, Repository,
    RoleFilter as DatabaseRoleFilter, RoleRepository, entities::role,
};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::{PersistenceFuture, RoleFilter, RoleReadPort, RoleRecord};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn RoleReadPort> {
    Arc::new(LegacyRoleRead { database })
}

struct LegacyRoleRead {
    database: ControlDatabaseCluster,
}

impl RoleReadPort for LegacyRoleRead {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<RoleRecord>> {
        Box::pin(async move {
            Ok(RoleRepository
                .find_by_id(self.database.write(), tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: RoleFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<RoleRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        include_super: bool,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<RoleRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: RoleFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<RoleRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_super_role<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<RoleRecord>> {
        Box::pin(async move {
            Ok(RoleRepository
                .find_super_role(self.database.write(), tenant_id)
                .await?
                .map(to_record))
        })
    }

    fn find_role_dept_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>> {
        Box::pin(async move {
            RoleRepository
                .find_role_dept_ids(self.database.write(), tenant_id, role_id)
                .await
        })
    }

    fn find_permission_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<String>>> {
        Box::pin(async move {
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
        })
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

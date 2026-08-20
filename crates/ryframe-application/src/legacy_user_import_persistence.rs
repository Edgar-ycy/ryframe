use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, DeptRepository, UserImportFilter, UserImportRepository, UserRepository,
    entities::{user_import_job, user_import_row_result},
};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{
    PersistenceFuture, UserImportDepartmentRecord, UserImportJobRecord, UserImportReadFilter,
    UserImportReadPort, UserImportRowRecord,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn UserImportReadPort> {
    Arc::new(LegacyUserImportRead { database })
}

struct LegacyUserImportRead {
    database: ControlDatabaseCluster,
}

impl UserImportReadPort for LegacyUserImportRead {
    fn list_departments<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<UserImportDepartmentRecord>> {
        Box::pin(async move {
            DeptRepository
                .find_filtered(self.database.write(), tenant_id, None, None)
                .await
                .map(|departments| {
                    departments
                        .into_iter()
                        .map(|department| UserImportDepartmentRecord {
                            id: department.id,
                            name: department.name,
                            parent_id: department.parent_id,
                            ancestors: department.ancestors,
                            status: department.status,
                        })
                        .collect()
                })
        })
    }

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: UserImportReadFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<UserImportJobRecord>> {
        Box::pin(async move {
            let result = UserImportRepository
                .list_for_tenant(
                    self.database.write(),
                    tenant_id,
                    &page,
                    UserImportFilter {
                        status: filter.status,
                    },
                )
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(job_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, Option<UserImportJobRecord>> {
        Box::pin(async move {
            UserImportRepository
                .find_by_id_for_tenant(self.database.write(), tenant_id, import_id)
                .await
                .map(|job| job.map(job_record))
        })
    }

    fn rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<UserImportRowRecord>> {
        Box::pin(async move {
            let result = UserImportRepository
                .list_row_results(self.database.write(), tenant_id, import_id, &page)
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(row_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn requester_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, String)>> {
        Box::pin(async move {
            UserRepository
                .find_usernames_by_ids(self.database.write(), tenant_id, user_ids)
                .await
        })
    }
}

fn job_record(job: user_import_job::Model) -> UserImportJobRecord {
    UserImportJobRecord {
        id: job.id,
        tenant_id: job.tenant_id,
        requester_user_id: job.requester_user_id,
        background_job_id: job.background_job_id,
        idempotency_key_hash: job.idempotency_key_hash,
        source_file_id: job.source_file_id,
        source_name_snapshot: job.source_name_snapshot,
        source_sha256: job.source_sha256,
        duplicate_policy: job.duplicate_policy,
        status: job.status,
        total_rows: job.total_rows,
        processed_rows: job.processed_rows,
        success_count: job.success_count,
        skipped_count: job.skipped_count,
        failure_count: job.failure_count,
        cancel_requested: job.cancel_requested,
        error_report_file_id: job.error_report_file_id,
        last_error: job.last_error,
        started_at: job.started_at,
        completed_at: job.completed_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

fn row_record(row: user_import_row_result::Model) -> UserImportRowRecord {
    UserImportRowRecord {
        row_number: row.row_number,
        username: row.username_snapshot,
        outcome: row.outcome,
        code: row.code,
        message: row.message,
        created_at: row.created_at,
    }
}

use std::sync::Arc;

use crate::{ControlDatabaseCluster, ReadConsistency, entities::service_access_audit};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};

use ryframe_application::{
    PersistenceFuture, ServiceAccessAuditRecord, ServiceAccountAuditReadPort,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ServiceAccountAuditReadPort> {
    Arc::new(DatabaseServiceAccountAuditPersistence { database })
}

struct DatabaseServiceAccountAuditPersistence {
    database: ControlDatabaseCluster,
}

impl ServiceAccountAuditReadPort for DatabaseServiceAccountAuditPersistence {
    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceAccessAuditRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            let total = service_access_audit::Entity::find()
                .filter(service_access_audit::Column::TenantId.eq(tenant_id))
                .count(&database)
                .await
                .map_err(database_error)?;
            let records = service_access_audit::Entity::find()
                .filter(service_access_audit::Column::TenantId.eq(tenant_id))
                .order_by_desc(service_access_audit::Column::StartedAt)
                .offset(page.offset())
                .limit(page.page_size())
                .all(&database)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(to_record)
                .collect();
            Ok(PageResult::new(records, total, &page))
        })
    }
}

fn to_record(audit: service_access_audit::Model) -> ServiceAccessAuditRecord {
    ServiceAccessAuditRecord {
        id: audit.id,
        request_id: audit.request_id,
        tenant_id: audit.tenant_id,
        account_id: audit.account_id,
        credential_id: audit.credential_id,
        delegation_id: audit.delegation_id,
        represented_user_id: audit.represented_user_id,
        operation_id: audit.operation_id,
        capability_key: audit.capability_key,
        required_permission: audit.required_permission,
        access_mode: audit.access_mode,
        result: audit.result,
        reason_code: audit.reason_code,
        http_status: audit.http_status,
        row_count: audit.row_count,
        response_bytes: audit.response_bytes,
        tenant_epoch: audit.tenant_epoch,
        account_authorization_version: audit.account_authorization_version,
        user_authorization_version: audit.user_authorization_version,
        delegation_version: audit.delegation_version,
        started_at: audit.started_at,
        completed_at: audit.completed_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

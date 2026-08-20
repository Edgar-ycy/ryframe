use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, DataRetentionRepository, ServiceAccessAuditRepository,
    entities::service_access_audit,
};
use ryframe_kernel::AppError;
use sea_orm::TransactionTrait;

use crate::{
    PersistenceFuture,
    agent::{AgentAccessAuditDraft, AgentAccessAuditRecord, AgentAuditWritePort},
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn AgentAuditWritePort> {
    Arc::new(LegacyAgentAuditWrite { database })
}

struct LegacyAgentAuditWrite {
    database: ControlDatabaseCluster,
}

impl AgentAuditWritePort for LegacyAgentAuditWrite {
    fn record_failure(&self, audit: AgentAccessAuditDraft) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            let result = async {
                let completed_at = DataRetentionRepository
                    .database_utc_now(&transaction)
                    .await?;
                let audit = audit.complete(completed_at);
                ServiceAccessAuditRepository
                    .insert(&transaction, model(audit))
                    .await?;
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    transaction.commit().await.map_err(database_error)?;
                    Ok(())
                }
                Err(error) => {
                    let _ = transaction.rollback().await;
                    Err(error)
                }
            }
        })
    }
}

pub(crate) fn model(audit: AgentAccessAuditRecord) -> service_access_audit::Model {
    let draft = audit.draft;
    service_access_audit::Model {
        id: draft.id,
        request_id: draft.request_id,
        tenant_id: draft.tenant_id,
        account_id: draft.account_id,
        credential_id: draft.credential_id,
        delegation_id: draft.delegation_id,
        represented_user_id: draft.represented_user_id,
        operation_id: draft.operation_id,
        capability_key: draft.capability_key,
        required_permission: draft.required_permission,
        access_mode: draft.access_mode,
        result: draft.result,
        reason_code: draft.reason_code,
        http_status: draft.http_status,
        request_ip_digest: draft.request_ip_digest,
        user_agent_digest: draft.user_agent_digest,
        row_count: draft.row_count,
        response_bytes: draft.response_bytes,
        tenant_epoch: draft.tenant_epoch,
        account_authorization_version: draft.account_authorization_version,
        user_authorization_version: draft.user_authorization_version,
        delegation_version: draft.delegation_version,
        started_at: draft.started_at,
        completed_at: audit.completed_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn audit_mapping_preserves_security_metadata() {
        let started_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let completed_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 4).unwrap();
        let record = AgentAccessAuditDraft {
            id: 1,
            request_id: "request-a".into(),
            tenant_id: Some("tenant-a".into()),
            account_id: Some(2),
            credential_id: Some(3),
            delegation_id: Some(4),
            represented_user_id: Some(5),
            operation_id: "agent.users".into(),
            capability_key: "directory.users".into(),
            required_permission: "system:user:list".into(),
            access_mode: "delegated".into(),
            result: "success".into(),
            reason_code: "ok".into(),
            http_status: 200,
            request_ip_digest: Some(vec![6, 7]),
            user_agent_digest: Some(vec![8, 9]),
            row_count: Some(10),
            response_bytes: Some(11),
            tenant_epoch: Some(12),
            account_authorization_version: Some(13),
            user_authorization_version: Some(14),
            delegation_version: Some(15),
            started_at,
        }
        .complete(completed_at);

        let model = model(record);

        assert_eq!(model.request_ip_digest, Some(vec![6, 7]));
        assert_eq!(model.user_agent_digest, Some(vec![8, 9]));
        assert_eq!(model.completed_at, completed_at);
        assert_eq!(model.delegation_version, Some(15));
    }
}

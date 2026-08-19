use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_application::system::{
    DataRetentionOverview as ServiceOverview, DataRetentionPolicy as ServicePolicy,
    DataRetentionPreview as ServicePreview, DataRetentionRunVo as ServiceRun,
};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct DataRetentionPolicy {
    pub cleanup_batch_size: usize,
    pub max_rows_per_resource_per_run: usize,
    pub background_job_succeeded_days: u32,
    pub outbox_published_days: u32,
    pub schedule_execution_days: u32,
    pub export_job_history_days: u32,
    pub operation_log_days: u32,
    pub login_log_days: u32,
    pub user_import_history_days: u32,
    pub user_import_artifact_hours: u32,
    pub tenant_config_artifact_hours: u32,
    pub tenant_config_rollback_hours: u32,
    pub service_access_audit_days: u32,
    pub retention_run_days: u32,
    pub dead_background_jobs_permanent: bool,
    pub dead_outbox_events_permanent: bool,
}

impl From<ServicePolicy> for DataRetentionPolicy {
    fn from(value: ServicePolicy) -> Self {
        Self {
            cleanup_batch_size: value.cleanup_batch_size,
            max_rows_per_resource_per_run: value.max_rows_per_resource_per_run,
            background_job_succeeded_days: value.background_job_succeeded_days,
            outbox_published_days: value.outbox_published_days,
            schedule_execution_days: value.schedule_execution_days,
            export_job_history_days: value.export_job_history_days,
            operation_log_days: value.operation_log_days,
            login_log_days: value.login_log_days,
            user_import_history_days: value.user_import_history_days,
            user_import_artifact_hours: value.user_import_artifact_hours,
            tenant_config_artifact_hours: value.tenant_config_artifact_hours,
            tenant_config_rollback_hours: value.tenant_config_rollback_hours,
            service_access_audit_days: value.service_access_audit_days,
            retention_run_days: value.retention_run_days,
            dead_background_jobs_permanent: value.dead_background_jobs_permanent,
            dead_outbox_events_permanent: value.dead_outbox_events_permanent,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataRetentionCutoff {
    pub resource: String,
    pub before: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataRetentionOverview {
    pub calculated_at: DateTime<Utc>,
    pub policy: DataRetentionPolicy,
    pub cutoffs: Vec<DataRetentionCutoff>,
}

impl From<ServiceOverview> for DataRetentionOverview {
    fn from(value: ServiceOverview) -> Self {
        Self {
            calculated_at: value.calculated_at,
            policy: value.policy.into(),
            cutoffs: value
                .cutoffs
                .into_iter()
                .map(|cutoff| DataRetentionCutoff {
                    resource: cutoff.resource,
                    before: cutoff.before,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataRetentionPreview {
    pub calculated_at: DateTime<Utc>,
    pub policy: DataRetentionPolicy,
    pub cutoffs: Vec<DataRetentionCutoff>,
    pub eligible_counts: BTreeMap<String, u64>,
}

impl From<ServicePreview> for DataRetentionPreview {
    fn from(value: ServicePreview) -> Self {
        Self {
            calculated_at: value.calculated_at,
            policy: value.policy.into(),
            cutoffs: value
                .cutoffs
                .into_iter()
                .map(|cutoff| DataRetentionCutoff {
                    resource: cutoff.resource,
                    before: cutoff.before,
                })
                .collect(),
            eligible_counts: value.eligible_counts,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataRetentionRunVo {
    pub id: String,
    pub background_job_id: String,
    pub trigger_kind: String,
    pub status: String,
    pub policy_snapshot: Value,
    pub eligible_counts: Value,
    pub deleted_counts: Value,
    pub remaining_counts: Value,
    pub requested_by: Option<String>,
    pub error_summary: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceRun> for DataRetentionRunVo {
    fn from(value: ServiceRun) -> Self {
        Self {
            id: value.id,
            background_job_id: value.background_job_id,
            trigger_kind: value.trigger_kind,
            status: value.status,
            policy_snapshot: value.policy_snapshot,
            eligible_counts: value.eligible_counts,
            deleted_counts: value.deleted_counts,
            remaining_counts: value.remaining_counts,
            requested_by: value.requested_by,
            error_summary: value.error_summary,
            started_at: value.started_at,
            completed_at: value.completed_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

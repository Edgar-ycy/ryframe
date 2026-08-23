use crate::{
    TenantConfigurationFence,
    entities::tenant::{
        config_bundle as tenant_config_bundle, config_transfer as tenant_config_transfer,
        config_transfer_item as tenant_config_transfer_item,
        operation_lease as tenant_operation_lease,
    },
};
use ryframe_application::ports::tenant_config::{
    TenantConfigBundleRecord, TenantConfigOperationLeaseRecord, TenantConfigTransferItemRecord,
    TenantConfigTransferRecord, TenantConfigurationFenceRecord,
};

impl From<TenantConfigurationFence> for TenantConfigurationFenceRecord {
    fn from(value: TenantConfigurationFence) -> Self {
        Self {
            configuration_version: value.configuration_version,
            authorization_epoch: value.authorization_epoch,
        }
    }
}

impl From<TenantConfigOperationLeaseRecord> for tenant_operation_lease::Model {
    fn from(value: TenantConfigOperationLeaseRecord) -> Self {
        Self {
            tenant_id: value.tenant_id,
            owner_token: value.owner_token,
            operation: value.operation,
            resource_type: value.resource_type,
            resource_id: value.resource_id,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_bundle::Model> for TenantConfigBundleRecord {
    fn from(value: tenant_config_bundle::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name_snapshot: value.source_tenant_name_snapshot,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            file_id: value.file_id,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            background_job_id: value.background_job_id,
            idempotency_key_hash: value.idempotency_key_hash,
            created_by: value.created_by,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigBundleRecord> for tenant_config_bundle::Model {
    fn from(value: TenantConfigBundleRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name_snapshot: value.source_tenant_name_snapshot,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            file_id: value.file_id,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            background_job_id: value.background_job_id,
            idempotency_key_hash: value.idempotency_key_hash,
            created_by: value.created_by,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_transfer::Model> for TenantConfigTransferRecord {
    fn from(value: tenant_config_transfer::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            bundle_id: value.bundle_id,
            idempotency_key_hash: value.idempotency_key_hash,
            request_kind: value.request_kind,
            request_fingerprint: value.request_fingerprint,
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch,
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            preview_background_job_id: value.preview_background_job_id,
            apply_background_job_id: value.apply_background_job_id,
            rollback_background_job_id: value.rollback_background_job_id,
            snapshot_file_id: value.snapshot_file_id,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value.applied_authorization_epoch,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            requested_by: value.requested_by,
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigTransferRecord> for tenant_config_transfer::Model {
    fn from(value: TenantConfigTransferRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            bundle_id: value.bundle_id,
            idempotency_key_hash: value.idempotency_key_hash,
            request_kind: value.request_kind,
            request_fingerprint: value.request_fingerprint,
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch,
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            preview_background_job_id: value.preview_background_job_id,
            apply_background_job_id: value.apply_background_job_id,
            rollback_background_job_id: value.rollback_background_job_id,
            snapshot_file_id: value.snapshot_file_id,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value.applied_authorization_epoch,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            requested_by: value.requested_by,
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_transfer_item::Model> for TenantConfigTransferItemRecord {
    fn from(value: tenant_config_transfer_item::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            transfer_id: value.transfer_id,
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigTransferItemRecord> for tenant_config_transfer_item::Model {
    fn from(value: TenantConfigTransferItemRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            transfer_id: value.transfer_id,
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

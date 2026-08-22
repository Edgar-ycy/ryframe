use chrono::{TimeZone, Utc};
use ryframe_application::ports::{
    retention::{RetentionResource, RetentionRunRecord},
    tenant_config::{
        TenantConfigBundleRecord, TenantConfigTransferItemRecord, TenantConfigTransferRecord,
        TenantConfigurationFenceRecord,
    },
};
use ryframe_db::{
    RetentionResource as DatabaseRetentionResource, TenantConfigurationFence,
    application_ports::{
        retention::{database_resource_key, retention_run_model, retention_run_record},
        tenant_config::{ACTIVE_TRANSFER_PREDICATE, INACTIVE_ROLLBACK_PREDICATE},
    },
    entities::{tenant_config_bundle, tenant_config_transfer, tenant_config_transfer_item},
};
use serde_json::json;

#[test]
fn active_transfer_predicate_fails_closed_for_every_running_state() {
    for status in [
        tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
        tenant_config_transfer::Model::STATUS_PREVIEWING,
        tenant_config_transfer::Model::STATUS_APPLY_PENDING,
        tenant_config_transfer::Model::STATUS_APPLYING,
    ] {
        assert!(ACTIVE_TRANSFER_PREDICATE.contains(status));
    }
}

#[test]
fn active_rollback_predicate_protects_pending_and_running_snapshots() {
    assert!(
        INACTIVE_ROLLBACK_PREDICATE
            .contains(tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING)
    );
    assert!(
        INACTIVE_ROLLBACK_PREDICATE.contains(tenant_config_transfer::Model::STATUS_ROLLING_BACK)
    );
}

#[test]
fn bundle_mapping_preserves_every_field() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 2, 3, 4).unwrap();
    let expected = tenant_config_bundle::Model {
        id: 7,
        tenant_id: "tenant-a".into(),
        origin: "uploaded".into(),
        source_tenant_key: "source-a".into(),
        source_tenant_name_snapshot: "来源租户".into(),
        package_schema_version: "1".into(),
        source_app_version: "0.10.0".into(),
        file_id: Some(8),
        sha256: Some("a".repeat(64)),
        resource_counts: json!({"role": 2}),
        item_count: 2,
        status: "succeeded".into(),
        background_job_id: Some(9),
        idempotency_key_hash: Some("b".repeat(64)),
        created_by: 10,
        error_summary: Some("摘要".into()),
        expires_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    let restored: tenant_config_bundle::Model =
        TenantConfigBundleRecord::from(expected.clone()).into();
    assert_eq!(restored, expected);
}

#[test]
fn transfer_mapping_preserves_every_field() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 3, 4, 5).unwrap();
    let expected = tenant_config_transfer::Model {
        id: 11,
        tenant_id: "tenant-a".into(),
        bundle_id: 12,
        idempotency_key_hash: "c".repeat(64),
        request_kind: "upload".into(),
        request_fingerprint: "d".repeat(64),
        status: "previewed".into(),
        target_configuration_version: 13,
        target_authorization_epoch: 14,
        plan_hash: Some("e".repeat(64)),
        preview_calculated_at: Some(now),
        preview_background_job_id: Some(15),
        apply_background_job_id: Some(16),
        rollback_background_job_id: Some(17),
        snapshot_file_id: Some(18),
        applied_configuration_version: Some(19),
        applied_authorization_epoch: Some(20),
        change_counts: json!({"create": 3}),
        error_summary: Some("错误".into()),
        requested_by: 21,
        rollback_expires_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    let restored: tenant_config_transfer::Model =
        TenantConfigTransferRecord::from(expected.clone()).into();
    assert_eq!(restored, expected);
}

#[test]
fn item_and_fence_mapping_preserve_boundaries() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 4, 5, 6).unwrap();
    let expected = tenant_config_transfer_item::Model {
        id: 22,
        tenant_id: "tenant-a".into(),
        transfer_id: 23,
        resource_type: "role".into(),
        stable_key: "role:admin".into(),
        display_name: "管理员".into(),
        action: "update".into(),
        outcome: "applied".into(),
        detail_code: Some("changed".into()),
        detail: Some("已更新".into()),
        created_at: now,
        updated_at: now,
    };
    let restored: tenant_config_transfer_item::Model =
        TenantConfigTransferItemRecord::from(expected.clone()).into();
    let fence = TenantConfigurationFenceRecord::from(TenantConfigurationFence {
        configuration_version: 24,
        authorization_epoch: 25,
    });

    assert_eq!(restored, expected);
    assert_eq!(fence.configuration_version, 24);
    assert_eq!(fence.authorization_epoch, 25);
}

#[test]
fn export_retention_never_bypasses_artifact_purge() {
    let predicate = DatabaseRetentionResource::ExportJobs.predicate();
    assert!(predicate.contains("delete_pending_at IS NULL"));
    assert!(predicate.contains("result_file_id IS NULL"));
}

#[test]
fn every_application_resource_maps_to_same_database_key() {
    for resource in RetentionResource::ALL {
        assert_eq!(resource.key(), database_resource_key(resource));
    }
}

#[test]
fn run_mapping_preserves_state() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 2, 3, 4).unwrap();
    let record = RetentionRunRecord {
        id: 1,
        background_job_id: 2,
        trigger_kind: RetentionRunRecord::TRIGGER_MANUAL.into(),
        status: RetentionRunRecord::STATUS_RUNNING.into(),
        policy_snapshot: json!({"policy": 1}),
        eligible_counts: json!({"files": 2}),
        deleted_counts: json!({"files": 1}),
        remaining_counts: json!({"files": 1}),
        requested_by: Some(3),
        error_summary: None,
        started_at: Some(now),
        completed_at: None,
        created_at: now,
        updated_at: now,
    };

    assert_eq!(
        retention_run_record(retention_run_model(record)).background_job_id,
        2
    );
}

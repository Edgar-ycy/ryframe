use ryframe_application::ports::tenant_data::{
    TenantDataCleanupOwnership as ApplicationCleanupOwnership, TenantDataTargetHealth,
};
use ryframe_db::entities::tenant::{
    data_migration as tenant_data_migration, data_migration_item as tenant_data_migration_item,
    data_placement as tenant_data_placement,
};
use ryframe_tenant_db::{
    TenantDataCleanupOwnership, TenantDatabaseTargetHealthStatus,
    application_ports::tenant_data::{
        map_cleanup_ownership, map_health, map_item, map_item_model, map_migration,
        map_migration_model, map_placement, map_placement_model,
    },
};
use time::{UtcOffset, macros::datetime};

#[test]
fn migration_mapping_preserves_every_field() {
    let now = datetime!(2026-08-21 06:32:03 +05:30);
    let later = datetime!(2026-08-22 04:05:06.123456 UTC);
    let model = tenant_data_migration::Model {
        id: 1,
        tenant_id: "tenant-a".into(),
        source_target_key: "source".into(),
        target_key: "target".into(),
        source_target_mode: "shared".into(),
        source_target_kind: "control".into(),
        target_target_mode: "dedicated".into(),
        target_target_kind: "external".into(),
        source_generation: 2,
        source_switch_token: "source-token".into(),
        target_generation: 3,
        source_schema_fingerprint: "source-schema".into(),
        target_schema_fingerprint: "target-schema".into(),
        plan_hash: "plan".into(),
        create_idempotency_key_hash: "create".into(),
        cancel_idempotency_key_hash: Some("cancel".into()),
        finalize_idempotency_key_hash: Some("finalize".into()),
        state: tenant_data_migration::Model::STATE_COPYING.into(),
        switch_token: "switch".into(),
        operator_id: 4,
        cancelled_by: Some(5),
        finalized_by: Some(6),
        background_job_id: Some(7),
        retention_hours: 168,
        error_code: Some("error".into()),
        error_detail: Some("detail".into()),
        prechecked_at: Some(now),
        queued_at: Some(now),
        quiesced_at: Some(now),
        frozen_at: Some(now),
        copy_started_at: Some(now),
        copy_completed_at: Some(later),
        verified_at: Some(later),
        cut_over_at: Some(later),
        activated_at: Some(later),
        succeeded_at: Some(later),
        retention_until: Some(later),
        cancel_requested_at: Some(now),
        finalize_requested_at: Some(later),
        cleanup_ready_at: Some(later),
        finalized_at: Some(later),
        failed_at: Some(later),
        cancelled_at: Some(later),
        created_at: now,
        updated_at: later,
    };

    let record = map_migration(model.clone());
    assert_eq!(record.created_at.offset(), now.offset());
    assert_eq!(map_migration_model(record), model);

    let mut legacy_json = serde_json::to_value(&model).unwrap();
    assert_eq!(legacy_json["created_at"], "2026-08-21T01:02:03Z");
    assert_eq!(
        legacy_json["retention_until"],
        "2026-08-22T04:05:06.123456Z"
    );
    legacy_json.as_object_mut().unwrap().remove("prechecked_at");
    legacy_json["queued_at"] = serde_json::Value::Null;
    let decoded: tenant_data_migration::Model = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(decoded.created_at, now);
    assert_eq!(decoded.created_at.offset(), UtcOffset::UTC);
    assert_eq!(decoded.prechecked_at, None);
    assert_eq!(decoded.queued_at, None);
}

#[test]
fn migration_item_and_placement_mapping_preserve_every_field() {
    let now = datetime!(2026-08-21 06:32:03.123 +05:30);
    let later = datetime!(2026-08-22 04:05:06.123456789 UTC);
    let item = tenant_data_migration_item::Model {
        id: 11,
        migration_id: 12,
        table_name: "sys_user".into(),
        copy_order: 13,
        state: tenant_data_migration_item::Model::STATE_VERIFIED.into(),
        cursor_json: Some(sea_orm::prelude::Json::Array(vec![
            sea_orm::prelude::Json::String("a".into()),
            sea_orm::prelude::Json::String("b".into()),
        ])),
        source_row_count: Some(14),
        target_row_count: Some(15),
        source_digest: Some("source".into()),
        target_digest: Some("target".into()),
        error_code: Some("error".into()),
        error_detail: Some("detail".into()),
        copy_started_at: Some(now),
        copied_at: Some(later),
        verified_at: Some(later),
        cleanup_state: tenant_data_migration_item::Model::CLEANUP_CLEANING.into(),
        cleanup_row_count: 16,
        created_at: now,
        updated_at: later,
    };
    let placement = tenant_data_placement::Model {
        tenant_id: "tenant-a".into(),
        current_target_key: "target".into(),
        placement_generation: 17,
        state: tenant_data_placement::Model::STATE_MAINTENANCE.into(),
        switch_token: "switch".into(),
        created_at: now,
        updated_at: later,
    };

    assert_eq!(map_item_model(map_item(item.clone())), item);
    assert_eq!(
        map_placement_model(map_placement(placement.clone())),
        placement
    );
    let item_json = serde_json::to_value(&item).unwrap();
    let placement_json = serde_json::to_value(&placement).unwrap();
    assert_eq!(item_json["copy_started_at"], "2026-08-21T01:02:03.123Z");
    assert_eq!(item_json["copied_at"], "2026-08-22T04:05:06.123456789Z");
    assert_eq!(placement_json["created_at"], "2026-08-21T01:02:03.123Z");
    assert_eq!(
        placement_json["updated_at"],
        "2026-08-22T04:05:06.123456789Z"
    );
}

#[test]
fn target_health_mapping_is_complete() {
    assert_eq!(
        map_health(TenantDatabaseTargetHealthStatus::Unknown),
        TenantDataTargetHealth::Unknown
    );
    assert_eq!(
        map_health(TenantDatabaseTargetHealthStatus::Verified),
        TenantDataTargetHealth::Verified
    );
    assert_eq!(
        map_health(TenantDatabaseTargetHealthStatus::Unavailable),
        TenantDataTargetHealth::Unavailable
    );
}

#[test]
fn cleanup_ownership_mapping_is_complete() {
    assert_eq!(
        map_cleanup_ownership(TenantDataCleanupOwnership::OwnedFrozen),
        ApplicationCleanupOwnership::OwnedFrozen
    );
    assert_eq!(
        map_cleanup_ownership(TenantDataCleanupOwnership::AlreadyClean),
        ApplicationCleanupOwnership::AlreadyClean
    );
    assert_eq!(
        map_cleanup_ownership(TenantDataCleanupOwnership::NotOwned),
        ApplicationCleanupOwnership::NotOwned
    );
}

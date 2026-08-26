use chrono::{DateTime, TimeZone, Utc};
use ryframe_application::ports::export::{CreateExportRecord, ExportStartDecision};
use ryframe_db::{
    ExportStartDisposition,
    application_ports::export::{database_create, map_start_decision},
    entities::{background_job, export_job},
    repositories::export_job_repo::{
        decide_export_start, validate_candidate_ownership, validate_deletion_candidates,
        visible_for_requester_query,
    },
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseBackend, QueryTrait};
use serde_json::json;

fn export(status: &str, tenant_id: &str, requester_id: i64) -> export_job::Model {
    let now = Utc::now();
    export_job::Model {
        id: 11,
        tenant_id: tenant_id.into(),
        requester_id,
        resource: "users".into(),
        background_job_id: 21,
        request_params: json!({}),
        request_version: 2,
        permission_code: "system:user:export".into(),
        authorization_fingerprint: "a".repeat(64),
        request_fingerprint: "b".repeat(64),
        active_request_fingerprint: None,
        snapshot_at: now,
        upper_id: 1,
        matched_rows: 1,
        exported_rows: 0,
        status: status.into(),
        result_file_id: None,
        result_file_name: None,
        content_type: None,
        file_size: None,
        expires_at: None,
        error_message: None,
        created_at: now,
        updated_at: now,
        completed_at: Some(now),
        notification_read_at: None,
        delete_pending_at: None,
    }
}

fn background(status: &str, lease_until: Option<DateTime<Utc>>) -> background_job::Model {
    let now = Utc::now();
    background_job::Model {
        id: 21,
        tenant_id: Some("tenant-a".into()),
        schedule_id: None,
        scheduled_for: None,
        max_runtime_seconds: None,
        job_type: "system.export.execute".into(),
        payload: json!({}),
        status: status.into(),
        priority: 0,
        available_at: now,
        attempts: 1,
        max_attempts: 3,
        lease_owner: lease_until.map(|_| "worker-a".into()),
        lease_until,
        dedupe_key: None,
        traceparent: None,
        tracestate: None,
        last_error: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
    }
}

#[test]
fn requester_queries_hide_delete_tombstones() {
    let statement = visible_for_requester_query("tenant-a", 7).build(DatabaseBackend::MySql);
    assert!(statement.sql.contains("delete_pending_at` IS NULL"));
}

#[test]
fn start_gate_rejects_duplicate_worker_and_third_tenant_export() {
    assert_eq!(
        decide_export_start(export_job::Model::STATUS_RUNNING, false, 1, 2),
        ExportStartDisposition::AlreadyRunning
    );
    assert_eq!(
        decide_export_start(export_job::Model::STATUS_QUEUED, false, 2, 2),
        ExportStartDisposition::ConcurrencyLimited
    );
    assert_eq!(
        decide_export_start(export_job::Model::STATUS_QUEUED, false, 1, 2),
        ExportStartDisposition::Started
    );
    assert_eq!(
        decide_export_start(export_job::Model::STATUS_QUEUED, true, 0, 2),
        ExportStartDisposition::NotRunnable
    );
}

#[test]
fn only_terminal_states_are_deletable() {
    let now = Utc::now();
    for status in [
        export_job::Model::STATUS_SUCCEEDED,
        export_job::Model::STATUS_FAILED,
        export_job::Model::STATUS_CANCELLED,
        export_job::Model::STATUS_EXPIRED,
    ] {
        validate_deletion_candidates(&[export(status, "tenant-a", 7)], &[], now)
            .expect("四种终态均应允许删除");
    }
    for status in [
        export_job::Model::STATUS_QUEUED,
        export_job::Model::STATUS_RUNNING,
    ] {
        assert!(matches!(
            validate_deletion_candidates(&[export(status, "tenant-a", 7)], &[], now),
            Err(AppError::Conflict(_))
        ));
    }
}

#[test]
fn active_lease_rejects_whole_batch_but_expired_lease_does_not() {
    let now = Utc::now();
    let exports = [
        export(export_job::Model::STATUS_SUCCEEDED, "tenant-a", 7),
        export(export_job::Model::STATUS_FAILED, "tenant-a", 7),
    ];
    let active = background(
        background_job::Model::STATUS_SUCCEEDED,
        Some(now + chrono::Duration::seconds(1)),
    );
    assert!(matches!(
        validate_deletion_candidates(&exports, &[active], now),
        Err(AppError::Conflict(_))
    ));

    let expired = background(
        background_job::Model::STATUS_RUNNING,
        Some(now - chrono::Duration::seconds(1)),
    );
    validate_deletion_candidates(&exports, &[expired], now).expect("过期租约不能永久阻止终态清理");
}

#[test]
fn foreign_missing_or_hidden_member_rejects_whole_batch_as_not_found() {
    assert!(matches!(
        validate_candidate_ownership(
            &[export(export_job::Model::STATUS_SUCCEEDED, "tenant-a", 7)],
            2,
            "tenant-a",
            7
        ),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        validate_candidate_ownership(
            &[export(export_job::Model::STATUS_SUCCEEDED, "tenant-a", 7)],
            1,
            "tenant-b",
            7
        ),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        validate_candidate_ownership(
            &[export(export_job::Model::STATUS_SUCCEEDED, "tenant-a", 7)],
            1,
            "tenant-a",
            8
        ),
        Err(AppError::NotFound(_))
    ));
    let mut hidden = export(export_job::Model::STATUS_SUCCEEDED, "tenant-a", 7);
    hidden.delete_pending_at = Some(Utc::now());
    assert!(matches!(
        validate_candidate_ownership(&[hidden], 1, "tenant-a", 7),
        Err(AppError::NotFound(_))
    ));
}

#[test]
fn create_mapping_moves_every_snapshot_field() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let command = database_create(CreateExportRecord {
        tenant_id: "tenant-a".into(),
        requester_id: 7,
        resource: "users".into(),
        background_job_id: 9,
        request_params: json!({"request_version": 2}),
        request_version: 2,
        permission_code: "system:user:export".into(),
        authorization_fingerprint: "authorization".into(),
        request_fingerprint: "request".into(),
        snapshot_at: now,
        upper_id: 99,
        matched_rows: 8,
    });

    assert_eq!(command.tenant_id, "tenant-a");
    assert_eq!(command.requester_id, 7);
    assert_eq!(command.resource, "users");
    assert_eq!(command.background_job_id, 9);
    assert_eq!(command.request_version, 2);
    assert_eq!(command.permission_code, "system:user:export");
    assert_eq!(command.authorization_fingerprint, "authorization");
    assert_eq!(command.request_fingerprint, "request");
    assert_eq!(command.snapshot_at, now);
    assert_eq!(command.upper_id, 99);
    assert_eq!(command.matched_rows, 8);
}

#[test]
fn every_start_disposition_maps_to_application_state() {
    assert_eq!(
        map_start_decision(ExportStartDisposition::Started),
        ExportStartDecision::Started
    );
    assert_eq!(
        map_start_decision(ExportStartDisposition::AlreadyRunning),
        ExportStartDecision::AlreadyRunning
    );
    assert_eq!(
        map_start_decision(ExportStartDisposition::ConcurrencyLimited),
        ExportStartDecision::ConcurrencyLimited
    );
    assert_eq!(
        map_start_decision(ExportStartDisposition::NotRunnable),
        ExportStartDecision::NotRunnable
    );
}

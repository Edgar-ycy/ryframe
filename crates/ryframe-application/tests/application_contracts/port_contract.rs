use std::{collections::BTreeSet, time::Duration};

use chrono::{TimeZone, Utc};
use ryframe_application::{
    ports::{
        health::{DependencyHealthCache, DependencyStatus},
        jobs::ExecutionTenantScope,
        retention::RetentionResource,
        tenant_config::{
            TENANT_CONFIG_PACKAGE_RESOURCE, TENANT_CONFIG_SNAPSHOT_RESOURCE,
            TenantConfigArtifactCounts,
        },
        users::{
            USER_QUERY_STATUS_NORMAL, UserImportAuthorizationSnapshot, UserImportJobRecord,
            UserQueryRecord,
        },
    },
    system::identity::{CaptchaStore, InMemoryCaptchaStore},
};

#[test]
fn dependency_health_cache_is_transport_neutral_and_fail_closed() {
    let cache = DependencyHealthCache::new(true, false, Duration::from_secs(30));
    let initial = cache.snapshot();
    assert!(initial.stale);
    assert_eq!(initial.mysql, DependencyStatus::Unknown);
    assert_eq!(initial.redis, DependencyStatus::Unknown);
    assert_eq!(initial.object_storage, DependencyStatus::NotRequired);

    cache.update(true, true, false);
    let current = cache.snapshot();
    assert!(!current.stale);
    assert!(current.is_ready());
}

fn user(status: &str) -> UserQueryRecord {
    UserQueryRecord {
        id: 1,
        username: "test".to_owned(),
        nickname: "测试".to_owned(),
        email: String::new(),
        phone: String::new(),
        avatar: None,
        status: status.to_owned(),
        dept_id: None,
        dept_name: None,
        remark: None,
        created_at: Utc::now(),
    }
}

#[test]
fn execution_scope_keeps_platform_semantics() {
    let all = ExecutionTenantScope::all();
    let fixed = ExecutionTenantScope::tenant_and_platform("tenant-a");
    assert_eq!(all.tenant_id(), None);
    assert_eq!(fixed.tenant_id(), Some("tenant-a"));
    assert_ne!(fixed, all);
}

#[test]
fn only_normal_user_status_is_enabled() {
    assert!(user(USER_QUERY_STATUS_NORMAL).is_enabled());
    assert!(!user("0").is_enabled());
    assert!(!user("2").is_enabled());
}

#[test]
fn user_import_terminal_statuses_are_application_owned() {
    let at = Utc.with_ymd_and_hms(2026, 8, 21, 8, 0, 0).unwrap();
    let mut record = UserImportJobRecord {
        id: 1,
        tenant_id: "tenant-a".into(),
        requester_user_id: 2,
        background_job_id: 3,
        idempotency_key_hash: "a".repeat(64),
        source_file_id: 4,
        source_name_snapshot: "users.xlsx".into(),
        source_sha256: "b".repeat(64),
        duplicate_policy: "skip_existing".into(),
        status: UserImportJobRecord::STATUS_RUNNING.into(),
        total_rows: 1,
        processed_rows: 0,
        success_count: 0,
        skipped_count: 0,
        failure_count: 0,
        cancel_requested: false,
        error_report_file_id: None,
        last_error: None,
        started_at: Some(at),
        completed_at: None,
        created_at: at,
        updated_at: at,
    };
    assert!(!record.is_terminal());
    record.status = UserImportJobRecord::STATUS_PARTIAL.into();
    assert!(record.is_terminal());
}

#[test]
fn user_import_authorization_requires_exact_versions() {
    let current = UserImportAuthorizationSnapshot {
        tenant_epoch: 2,
        tenant_available: true,
        requester_enabled: true,
        requester_version: Some(3),
    };
    assert!(current.matches(2, 3));
    assert!(!current.matches(1, 3));
    assert!(!current.matches(2, 4));
    assert!(
        !UserImportAuthorizationSnapshot {
            requester_enabled: false,
            ..current
        }
        .matches(2, 3)
    );
}

#[test]
fn retention_resource_keys_are_complete_and_unique() {
    let keys = RetentionResource::ALL
        .into_iter()
        .map(RetentionResource::key)
        .collect::<BTreeSet<_>>();
    assert_eq!(keys.len(), RetentionResource::ALL.len());
}

#[test]
fn tenant_config_counts_use_stable_resource_keys() {
    let counts = TenantConfigArtifactCounts {
        packages: 2,
        snapshots: 3,
    }
    .into_resource_counts();
    assert_eq!(counts[TENANT_CONFIG_PACKAGE_RESOURCE], 2);
    assert_eq!(counts[TENANT_CONFIG_SNAPSHOT_RESOURCE], 3);
}

#[tokio::test]
async fn in_memory_captcha_is_case_insensitive_and_one_time() {
    let store = InMemoryCaptchaStore::new(60);
    store
        .set("captcha-a".into(), "Ab12".into())
        .await
        .expect("验证码应保存成功");
    assert!(store.verify("captcha-a", "aB12").await.unwrap());
    assert!(!store.verify("captcha-a", "aB12").await.unwrap());
}

use super::*;
use crate::{
    AuthorizationCache, AuthorizationCacheBackend, AuthorizationCacheLookup,
    AuthorizationChangePublisher, AuthorizationSnapshot, CacheAvailabilityPolicy,
    NamespaceCacheLookup, PersistenceTransaction, TenantCacheLookup, TransactionAuditMode,
    ports::{
        authorization::{AuthorizationMirrorEvent, AuthorizationMirrorTransaction},
        jobs::BackgroundJobTransaction,
        tenant_data::*,
    },
};
use async_trait::async_trait;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use time::OffsetDateTime;

#[path = "activation_stage.rs"]
mod stage;

#[derive(Clone, Debug, PartialEq)]
struct State {
    migration: TenantDataMigrationRecord,
    placement: TenantDataPlacementRecord,
    runtime: i64,
    scope: i64,
    epoch: i32,
    mirrors: Vec<serde_json::Value>,
}

#[derive(Clone, Copy, PartialEq)]
enum Failure {
    None,
    Save,
    Outbox,
    CommitBeforeWrite,
    Mirror,
}

struct Shared {
    persisted: Mutex<State>,
    events: Mutex<Vec<String>>,
    mirrored: AtomicI32,
    failure: Failure,
    commit_attempted: AtomicBool,
}

struct Work {
    shared: Arc<Shared>,
    pending: Mutex<State>,
    cache: AuthorizationCache,
    finished: bool,
}

impl Work {
    fn new(shared: &Arc<Shared>, cache: &AuthorizationCache) -> Box<Self> {
        Box::new(Self {
            pending: Mutex::new(shared.persisted.lock().unwrap().clone()),
            shared: shared.clone(),
            cache: cache.clone(),
            finished: false,
        })
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if !self.finished {
            self.shared.events.lock().unwrap().push("rollback".into());
        }
    }
}

fn fixture(failure: Failure) -> (Arc<Shared>, AuthorizationCache) {
    let now = chrono::DateTime::from_timestamp(1_800_000_000, 123_456_000).unwrap();
    let migration = TenantDataMigrationRecord {
        id: 55,
        tenant_id: "migration-tenant".into(),
        source_target_key: "old".into(),
        target_key: "new".into(),
        source_target_mode: "dedicated".into(),
        source_target_kind: "mysql".into(),
        target_target_mode: "dedicated".into(),
        target_target_kind: "mysql".into(),
        source_generation: 1,
        source_switch_token: "a".repeat(64),
        target_generation: 2,
        source_schema_fingerprint: "c".repeat(64),
        target_schema_fingerprint: "c".repeat(64),
        plan_hash: "d".repeat(64),
        create_idempotency_key_hash: "e".repeat(64),
        cancel_idempotency_key_hash: None,
        finalize_idempotency_key_hash: None,
        state: TenantDataMigrationRecord::STATE_ACTIVATING.into(),
        switch_token: "b".repeat(64),
        operator_id: 1,
        cancelled_by: None,
        finalized_by: None,
        background_job_id: Some(56),
        retention_hours: 168,
        error_code: None,
        error_detail: None,
        prechecked_at: Some(now),
        queued_at: Some(now),
        quiesced_at: Some(now),
        frozen_at: Some(now),
        copy_started_at: Some(now),
        copy_completed_at: Some(now),
        verified_at: Some(now),
        cut_over_at: Some(now),
        activated_at: None,
        succeeded_at: None,
        retention_until: None,
        cancel_requested_at: None,
        finalize_requested_at: None,
        cleanup_ready_at: None,
        finalized_at: None,
        failed_at: None,
        cancelled_at: None,
        created_at: now,
        updated_at: now,
    };
    let placement = TenantDataPlacementRecord {
        tenant_id: migration.tenant_id.clone(),
        current_target_key: migration.target_key.clone(),
        placement_generation: 2,
        state: TenantDataPlacementRecord::STATE_MAINTENANCE.into(),
        switch_token: migration.switch_token.clone(),
        created_at: now,
        updated_at: now,
    };
    let shared = Arc::new(Shared {
        persisted: Mutex::new(State {
            migration,
            placement,
            runtime: 3,
            scope: 9,
            epoch: 7,
            mirrors: vec![],
        }),
        events: Mutex::new(vec![]),
        mirrored: AtomicI32::new(7),
        failure,
        commit_attempted: AtomicBool::new(false),
    });
    let cache = AuthorizationCache::new(
        Some(shared.clone()),
        Some(shared.clone()),
        CacheAvailabilityPolicy::Required,
    );
    (shared, cache)
}

#[tokio::test]
async fn activation_preserves_published_scope_and_commits_epoch_once() {
    let (shared, cache) = fixture(Failure::None);
    let before = shared.persisted.lock().unwrap().clone();
    let result = activate(
        Work::new(&shared, &cache),
        &before.migration,
        before.scope,
        before.migration.updated_at,
        &cache,
    )
    .await
    .unwrap();
    assert_eq!(result.state, TenantDataMigrationRecord::STATE_SUCCEEDED);
    let committed = shared.persisted.lock().unwrap().clone();
    assert_eq!(
        (committed.runtime, committed.scope, committed.epoch),
        (4, 9, 8)
    );
    assert_eq!(
        committed.placement.state,
        TenantDataPlacementRecord::STATE_ACTIVE
    );
    assert_eq!(
        committed.mirrors,
        vec![
            serde_json::json!({"scope":"tenant", "tenant_id":"migration-tenant", "authorization_epoch":8})
        ]
    );
    assert_eq!(shared.mirrored.load(Ordering::SeqCst), 8);
    assert_eq!(
        *shared.events.lock().unwrap(),
        ["commit", "mirror:8", "publish:8"]
    );
    activate(
        Work::new(&shared, &cache),
        &result,
        committed.scope,
        result.updated_at,
        &cache,
    )
    .await
    .unwrap();
    assert_eq!(*shared.persisted.lock().unwrap(), committed);
    assert_eq!(
        *shared.events.lock().unwrap(),
        ["commit", "mirror:8", "publish:8", "commit"]
    );
}

#[tokio::test]
async fn activation_failure_rolls_back_versions_and_never_replays_committed_mirror_failure() {
    for failure in [
        Failure::Save,
        Failure::Outbox,
        Failure::CommitBeforeWrite,
        Failure::Mirror,
    ] {
        let (shared, cache) = fixture(failure);
        let before = shared.persisted.lock().unwrap().clone();
        assert!(
            activate(
                Work::new(&shared, &cache),
                &before.migration,
                before.scope,
                before.migration.updated_at,
                &cache
            )
            .await
            .is_err()
        );
        let after = shared.persisted.lock().unwrap().clone();
        assert_eq!(shared.mirrored.load(Ordering::SeqCst), 7);
        if failure == Failure::Mirror {
            assert_eq!((after.runtime, after.scope, after.epoch), (4, 9, 8));
            assert_eq!(
                after.migration.state,
                TenantDataMigrationRecord::STATE_SUCCEEDED
            );
            assert_eq!(after.mirrors.len(), 1);
            assert_eq!(*shared.events.lock().unwrap(), ["commit", "mirror-failed"]);
            activate(
                Work::new(&shared, &cache),
                &after.migration,
                after.scope,
                after.migration.updated_at,
                &cache,
            )
            .await
            .unwrap();
            assert_eq!(*shared.persisted.lock().unwrap(), after);
            assert_eq!(
                *shared.events.lock().unwrap(),
                ["commit", "mirror-failed", "commit"]
            );
        } else {
            assert_eq!(after, before);
            assert_eq!(*shared.events.lock().unwrap(), ["rollback"]);
        }
    }
}

#[async_trait]
impl PersistenceTransaction for Work {
    async fn commit(mut self: Box<Self>, audit: TransactionAuditMode) -> AppResult<()> {
        assert_eq!(audit, TransactionAuditMode::Skip);
        if self.shared.failure == Failure::CommitBeforeWrite
            && !self.shared.commit_attempted.swap(true, Ordering::SeqCst)
        {
            return Err(AppError::Database("合成提交前失败".into()));
        }
        *self.shared.persisted.lock().unwrap() = self.pending.lock().unwrap().clone();
        self.shared.events.lock().unwrap().push("commit".into());
        self.finished = true;
        Ok(())
    }
    async fn rollback(mut self: Box<Self>) -> AppResult<()> {
        self.shared.events.lock().unwrap().push("rollback".into());
        self.finished = true;
        Ok(())
    }
}

#[async_trait]
impl crate::DataScopeMutationTransaction for Work {
    async fn record_data_scope_change(&self, tenant_id: &str) -> AppResult<i64> {
        let mut state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        state.scope += 1;
        Ok(state.scope)
    }
}

#[async_trait]
impl AuthorizationMirrorTransaction for Work {
    async fn increment_tenant_epoch<'a>(&'a self, tenant_id: &'a str) -> AppResult<i32> {
        let mut state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        state.epoch += 1;
        Ok(state.epoch)
    }
    async fn database_now(&self) -> AppResult<OffsetDateTime> {
        let now = self.pending.lock().unwrap().migration.updated_at;
        Ok(OffsetDateTime::from_unix_timestamp(now.timestamp())
            .and_then(|time| time.replace_nanosecond(now.timestamp_subsec_nanos()))
            .unwrap())
    }
    async fn record(&self, event: AuthorizationMirrorEvent) -> AppResult<()> {
        if self.shared.failure == Failure::Outbox {
            return Err(AppError::Database("合成 outbox 失败".into()));
        }
        let mut pending = self.pending.lock().unwrap();
        let now = pending.migration.updated_at;
        assert_eq!(event.available_at.unix_timestamp(), now.timestamp());
        assert_eq!(
            event.available_at.nanosecond(),
            now.timestamp_subsec_nanos()
        );
        pending.mirrors.push(event.payload);
        Ok(())
    }
    async fn increment_user_versions<'a>(&'a self, _: &'a str, _: &'a [i64]) -> AppResult<u64> {
        unreachable!()
    }
    async fn user_versions<'a>(&'a self, _: &'a str, _: &'a [i64]) -> AppResult<Vec<(i64, i32)>> {
        unreachable!()
    }
    async fn increment_namespace_version<'a>(&'a self, _: &'a str, _: &'a str) -> AppResult<i64> {
        unreachable!()
    }
}

#[async_trait]
impl TenantDataMigrationTransaction for Work {
    fn background_jobs(&self) -> &dyn BackgroundJobTransaction {
        unreachable!()
    }
    async fn database_now(&self) -> AppResult<OffsetDateTime> {
        AuthorizationMirrorTransaction::database_now(self).await
    }
    async fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        owner: Option<&'a str>,
    ) -> AppResult<TenantMigrationContextRecord> {
        let state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        assert_eq!(owner, Some(state.migration.switch_token.as_str()));
        Ok(TenantMigrationContextRecord {
            authorization_epoch: state.epoch,
            data_scope_version: state.scope,
        })
    }
    async fn lock_placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> AppResult<TenantDataPlacementRecord> {
        let state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.placement.tenant_id);
        Ok(state.placement.clone())
    }
    async fn save_placement(
        &self,
        value: TenantDataPlacementRecord,
    ) -> AppResult<TenantDataPlacementRecord> {
        self.pending.lock().unwrap().placement = value.clone();
        Ok(value)
    }
    async fn lock_migration(&self, id: i64) -> AppResult<TenantDataMigrationRecord> {
        let state = self.pending.lock().unwrap();
        assert_eq!(id, state.migration.id);
        Ok(state.migration.clone())
    }
    async fn save_migration(
        &self,
        value: TenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        if self.shared.failure == Failure::Save {
            return Err(AppError::Database("合成状态保存失败".into()));
        }
        self.pending.lock().unwrap().migration = value.clone();
        Ok(value)
    }
    async fn increment_runtime_epoch<'a>(&'a self, tenant_id: &'a str) -> AppResult<i64> {
        let mut state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        state.runtime += 1;
        Ok(state.runtime)
    }
    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32> {
        self.cache
            .increment_tenant_epoch_in_transaction(self, tenant_id)
            .await
    }
    async fn acquire_lease(&self, _: TenantOperationLeaseRecord) -> AppResult<()> {
        unreachable!()
    }
    async fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner: &'a str,
        expires_at: OffsetDateTime,
    ) -> AppResult<bool> {
        let state = self.pending.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        assert_eq!(owner, state.migration.switch_token);
        let now = crate::ports::utc_timestamp::from_chrono(state.migration.updated_at).unwrap();
        assert_eq!(expires_at, now + time::Duration::hours(24));
        assert_eq!(expires_at.offset(), time::UtcOffset::UTC);
        self.shared.events.lock().unwrap().push("renew".into());
        Ok(true)
    }
    async fn release_lease<'a>(&'a self, _: &'a str, _: &'a str) -> AppResult<bool> {
        unreachable!()
    }
    async fn lock_active_migration_for_tenant<'a>(
        &'a self,
        _: &'a str,
    ) -> AppResult<Option<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn insert_migration(
        &self,
        _: CreateTenantDataMigrationRecord,
    ) -> AppResult<TenantDataMigrationRecord> {
        unreachable!()
    }
    async fn lock_item(&self, _: i64) -> AppResult<TenantDataMigrationItemRecord> {
        unreachable!()
    }
    async fn save_item(
        &self,
        _: TenantDataMigrationItemRecord,
    ) -> AppResult<TenantDataMigrationItemRecord> {
        unreachable!()
    }
    async fn has_validated_backup<'a>(
        &'a self,
        _: &'a TenantDataMigrationRecord,
        _: OffsetDateTime,
        _: OffsetDateTime,
    ) -> AppResult<bool> {
        unreachable!()
    }
}

#[async_trait]
impl AuthorizationCacheBackend for Shared {
    async fn update_tenant_epoch(&self, tenant_id: &str, epoch: i32) -> Result<(), String> {
        let state = self.persisted.lock().unwrap();
        assert_eq!(tenant_id, state.migration.tenant_id);
        assert_eq!(epoch, state.epoch);
        if self.failure == Failure::Mirror {
            self.events.lock().unwrap().push("mirror-failed".into());
            return Err("合成镜像失败".into());
        }
        self.mirrored.store(epoch, Ordering::SeqCst);
        self.events.lock().unwrap().push(format!("mirror:{epoch}"));
        Ok(())
    }
    async fn lookup_snapshot(&self, _: &str, _: i64) -> Result<AuthorizationCacheLookup, String> {
        unreachable!()
    }
    async fn store_snapshot(&self, _: &AuthorizationSnapshot) -> Result<bool, String> {
        unreachable!()
    }
    async fn update_user_version(&self, _: &str, _: i64, _: i32) -> Result<(), String> {
        unreachable!()
    }
    async fn read_tenant_value(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<TenantCacheLookup>, String> {
        unreachable!()
    }
    async fn store_tenant_value(
        &self,
        _: &str,
        _: &str,
        _: i32,
        _: &str,
        _: u64,
    ) -> Result<bool, String> {
        unreachable!()
    }
    async fn update_namespace_version(&self, _: &str, _: &str, _: i64) -> Result<(), String> {
        unreachable!()
    }
    async fn read_namespace_value(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<Option<NamespaceCacheLookup>, String> {
        unreachable!()
    }
    async fn store_namespace_value(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: i64,
        _: &str,
        _: u64,
    ) -> Result<bool, String> {
        unreachable!()
    }
}

#[async_trait]
impl AuthorizationChangePublisher for Shared {
    async fn publish(&self, _: &str, payload: &str) -> Result<(), String> {
        let event: serde_json::Value = serde_json::from_str(payload).unwrap();
        let epoch = self.persisted.lock().unwrap().epoch;
        assert_eq!(event["tenant_id"], "migration-tenant");
        assert_eq!(event["authorization_epoch"], epoch);
        assert_eq!(self.mirrored.load(Ordering::SeqCst), epoch);
        self.events.lock().unwrap().push(format!("publish:{epoch}"));
        Ok(())
    }
}

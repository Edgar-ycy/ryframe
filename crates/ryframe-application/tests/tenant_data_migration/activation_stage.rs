use super::*;
use crate::{DataScopeProjectionEvent, DataScopeProjectionPublisher};
use std::collections::HashSet;

struct Persistence {
    shared: Arc<Shared>,
    cache: AuthorizationCache,
}

struct Publisher {
    shared: Arc<Shared>,
    published: Mutex<Option<i64>>,
    fail: bool,
}

#[async_trait]
impl TenantDataMigrationPort for Publisher {
    fn catalog_tables(&self) -> Vec<TenantDataCatalogTable> {
        unreachable!()
    }
    async fn activate_fence(&self, fence: TenantDataFence<'_>) -> AppResult<()> {
        let state = self.shared.persisted.lock().unwrap();
        assert_eq!(fence.target_key, state.migration.target_key);
        assert_eq!(fence.generation, state.migration.target_generation);
        assert_eq!(fence.switch_token, state.migration.switch_token);
        self.shared.events.lock().unwrap().push("fence".into());
        Ok(())
    }
    async fn prepare_target(&self, _: TenantDataFence<'_>) -> AppResult<()> {
        unreachable!()
    }
    async fn clear_prepared_target(&self, _: TenantDataFence<'_>) -> AppResult<()> {
        unreachable!()
    }
    async fn freeze_fence(&self, _: TenantDataFence<'_>) -> AppResult<()> {
        unreachable!()
    }
    async fn assert_frozen_fence(&self, _: TenantDataFence<'_>) -> AppResult<()> {
        unreachable!()
    }
    async fn cleanup_ownership(
        &self,
        _: TenantDataFence<'_>,
    ) -> AppResult<TenantDataCleanupOwnership> {
        unreachable!()
    }
    async fn delete_rows_batch(&self, _: TenantDataFence<'_>, _: &str, _: u32) -> AppResult<u64> {
        unreachable!()
    }
    async fn finish_cleanup(&self, _: TenantDataFence<'_>) -> AppResult<()> {
        unreachable!()
    }
    async fn read_rows_batch(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&[String]>,
        _: u32,
    ) -> AppResult<TenantDataRowBatch> {
        unreachable!()
    }
    async fn write_rows_batch(
        &self,
        _: TenantDataFence<'_>,
        _: &str,
        _: &[TenantDataRow],
    ) -> AppResult<()> {
        unreachable!()
    }
    async fn verify_foreign_keys(&self, _: &str, _: &str, _: &str) -> AppResult<()> {
        unreachable!()
    }
}

#[async_trait]
impl DataScopeProjectionPublisher for Publisher {
    async fn publish(&self, _: &DataScopeProjectionEvent) -> AppResult<()> {
        unreachable!()
    }

    async fn publish_for_migration(&self, fence: TenantDataFence<'_>) -> AppResult<i64> {
        let state = self.shared.persisted.lock().unwrap();
        assert_eq!(
            state.placement.state,
            TenantDataPlacementRecord::STATE_MAINTENANCE
        );
        assert_eq!(fence.tenant_id, state.migration.tenant_id);
        assert_eq!(fence.target_key, state.migration.target_key);
        assert_eq!(fence.generation, state.migration.target_generation);
        assert_eq!(fence.switch_token, state.migration.switch_token);
        if self.fail {
            self.shared
                .events
                .lock()
                .unwrap()
                .push("projection-failed".into());
            return Err(AppError::Conflict("合成实际投影核验失败".into()));
        }
        let mut published = self.published.lock().unwrap();
        let action = if published.is_some() {
            "verify"
        } else {
            "projection"
        };
        self.shared
            .events
            .lock()
            .unwrap()
            .push(format!("{action}:{}", state.scope));
        *published = Some(state.scope);
        Ok(state.scope)
    }
}

#[tokio::test]
async fn projection_must_succeed_before_the_activation_transaction_starts() {
    for fail in [true, false] {
        let (shared, cache) = fixture(Failure::None);
        let before = shared.persisted.lock().unwrap().clone();
        let persistence = Persistence {
            shared: shared.clone(),
            cache: cache.clone(),
        };
        let publisher = Publisher {
            shared: shared.clone(),
            published: Mutex::new(None),
            fail,
        };
        let result = activate_control(
            &persistence,
            &publisher,
            &publisher,
            &cache,
            before.migration.clone(),
        )
        .await;
        if fail {
            assert!(
                matches!(result, Err(AppError::Conflict(ref detail)) if detail.contains("投影核验失败"))
            );
            assert_eq!(*shared.persisted.lock().unwrap(), before);
            assert_eq!(
                *shared.events.lock().unwrap(),
                ["fence", "projection-failed"]
            );
        } else {
            assert_eq!(
                result.unwrap().state,
                TenantDataMigrationRecord::STATE_SUCCEEDED
            );
            assert_eq!(
                *shared.events.lock().unwrap(),
                [
                    "fence",
                    "projection:9",
                    "begin",
                    "renew",
                    "commit",
                    "mirror:8",
                    "publish:8"
                ]
            );
            assert_eq!(shared.persisted.lock().unwrap().scope, before.scope);
        }
    }
}

#[tokio::test]
async fn published_target_is_reverified_after_control_commit_failure() {
    let (shared, cache) = fixture(Failure::CommitBeforeWrite);
    let before = shared.persisted.lock().unwrap().clone();
    let persistence = Persistence {
        shared: shared.clone(),
        cache: cache.clone(),
    };
    let publisher = Publisher {
        shared: shared.clone(),
        published: Mutex::new(None),
        fail: false,
    };
    assert!(
        activate_control(
            &persistence,
            &publisher,
            &publisher,
            &cache,
            before.migration.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(*shared.persisted.lock().unwrap(), before);
    assert_eq!(*publisher.published.lock().unwrap(), Some(9));
    activate_control(
        &persistence,
        &publisher,
        &publisher,
        &cache,
        before.migration,
    )
    .await
    .unwrap();
    assert_eq!(
        *shared.events.lock().unwrap(),
        [
            "fence",
            "projection:9",
            "begin",
            "renew",
            "rollback",
            "fence",
            "verify:9",
            "begin",
            "renew",
            "commit",
            "mirror:8",
            "publish:8",
        ]
    );
    let after = shared.persisted.lock().unwrap();
    assert_eq!(
        after.placement.state,
        TenantDataPlacementRecord::STATE_ACTIVE
    );
    assert_eq!((after.scope, after.runtime, after.epoch), (9, 4, 8));
}

#[tokio::test]
async fn activation_rejects_scope_placement_and_original_fence_drift() {
    for drift in ["scope", "placement", "fence", "state"] {
        let (shared, cache) = fixture(Failure::None);
        let snapshot = shared.persisted.lock().unwrap().migration.clone();
        {
            let mut state = shared.persisted.lock().unwrap();
            match drift {
                "scope" => state.scope += 1,
                "placement" => state.placement.placement_generation += 1,
                "fence" => {
                    state.migration.target_key = "drift".into();
                    state.placement.current_target_key = "drift".into();
                }
                "state" => state.placement.state = TenantDataPlacementRecord::STATE_ACTIVE.into(),
                _ => unreachable!(),
            }
        }
        let before = shared.persisted.lock().unwrap().clone();
        let error = activate(
            Work::new(&shared, &cache),
            &snapshot,
            9,
            snapshot.updated_at,
            &cache,
        )
        .await
        .unwrap_err();
        match drift {
            "scope" => assert!(
                matches!(error, AppError::Conflict(ref detail) if detail.contains("数据范围版本"))
            ),
            "state" => assert!(
                matches!(error, AppError::TenantOperationConflict(ref detail) if detail.contains("维护状态"))
            ),
            _ => assert!(
                matches!(error, AppError::StalePlacementGeneration(ref detail) if detail.contains("围栏身份"))
            ),
        }
        assert_eq!(*shared.persisted.lock().unwrap(), before);
        assert_eq!(*shared.events.lock().unwrap(), ["rollback"]);
    }
}

#[async_trait]
impl TenantDataMigrationPersistencePort for Persistence {
    async fn database_now(&self) -> AppResult<OffsetDateTime> {
        let now = self.shared.persisted.lock().unwrap().migration.updated_at;
        Ok(crate::ports::utc_timestamp::from_chrono(now).unwrap())
    }
    async fn begin(&self) -> AppResult<Box<dyn TenantDataMigrationTransaction>> {
        self.shared.events.lock().unwrap().push("begin".into());
        Ok(Work::new(&self.shared, &self.cache))
    }
    async fn occupied_target_keys<'a>(&'a self, _: &'a [String]) -> AppResult<HashSet<String>> {
        unreachable!()
    }
    async fn placement<'a>(&'a self, _: &'a str) -> AppResult<Option<TenantDataPlacementRecord>> {
        unreachable!()
    }
    async fn migration(&self, _: i64) -> AppResult<Option<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn migrations_for_tenant<'a>(
        &'a self,
        _: &'a str,
        _: u64,
    ) -> AppResult<Vec<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn recoverable_migrations(
        &self,
        _: Option<i64>,
        _: u64,
    ) -> AppResult<Vec<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn migration_by_create_key<'a>(
        &'a self,
        _: &'a str,
    ) -> AppResult<Option<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn active_migration_for_tenant<'a>(
        &'a self,
        _: &'a str,
    ) -> AppResult<Option<TenantDataMigrationRecord>> {
        unreachable!()
    }
    async fn items(&self, _: i64) -> AppResult<Vec<TenantDataMigrationItemRecord>> {
        unreachable!()
    }
    async fn insert_item(
        &self,
        _: TenantDataMigrationItemRecord,
    ) -> AppResult<TenantDataMigrationItemRecord> {
        unreachable!()
    }
    async fn save_item(
        &self,
        _: TenantDataMigrationItemRecord,
    ) -> AppResult<TenantDataMigrationItemRecord> {
        unreachable!()
    }
    async fn backup_points_for_target<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
        _: u64,
    ) -> AppResult<Vec<TenantDataBackupPointRecord>> {
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

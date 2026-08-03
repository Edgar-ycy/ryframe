mod common;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use ryframe_config::JobConfig;
use ryframe_db::{
    DatabaseCluster,
    entities::{export_job, sys_file},
};
use ryframe_kernel::AppError;
use ryframe_service::{
    AuthorizationCache,
    system::{ExportService, UserService},
};
use ryframe_storage::{ObjectStorage, StorageError, StorageResult};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Schema, TransactionTrait, sea_query::Expr,
};

const TENANT_ID: &str = "system";
const EXPORT_COUNT: i64 = 205;

#[derive(Clone)]
struct SeededExport {
    export_id: i64,
    file_id: i64,
    key: String,
}

struct ConcurrentCleanup {
    database: DatabaseConnection,
    export_id: i64,
    file_id: i64,
}

#[derive(Default)]
struct StorageState {
    objects: HashMap<String, Vec<u8>>,
    fail_delete_once: HashSet<String>,
    concurrent_cleanup: HashMap<String, ConcurrentCleanup>,
}

#[derive(Default)]
struct ControlledObjectStorage {
    state: Mutex<StorageState>,
}

impl ControlledObjectStorage {
    fn state(&self) -> MutexGuard<'_, StorageState> {
        self.state.lock().expect("锁定测试对象存储状态")
    }

    fn location(bucket: &str, key: &str) -> String {
        format!("{bucket}/{key}")
    }

    fn seed(&self, bucket: &str, key: &str, data: Vec<u8>) {
        self.state()
            .objects
            .insert(Self::location(bucket, key), data);
    }

    fn fail_delete_once(&self, bucket: &str, key: &str) {
        self.state()
            .fail_delete_once
            .insert(Self::location(bucket, key));
    }

    fn complete_concurrently_on_delete(
        &self,
        bucket: &str,
        key: &str,
        database: DatabaseConnection,
        export_id: i64,
        file_id: i64,
    ) {
        self.state().concurrent_cleanup.insert(
            Self::location(bucket, key),
            ConcurrentCleanup {
                database,
                export_id,
                file_id,
            },
        );
    }

    fn object_count(&self) -> usize {
        self.state().objects.len()
    }
}

#[async_trait]
impl ObjectStorage for ControlledObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        self.state()
            .objects
            .insert(Self::location(bucket, key), data.to_vec());
        Ok(())
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        self.state()
            .objects
            .get(&Self::location(bucket, key))
            .cloned()
            .ok_or_else(|| StorageError::Service {
                operation: "GET",
                status: 404,
                message: "测试对象不存在".into(),
            })
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let location = Self::location(bucket, key);
        let concurrent_cleanup = {
            let mut state = self.state();
            if state.fail_delete_once.remove(&location) {
                return Err(StorageError::Service {
                    operation: "DELETE",
                    status: 503,
                    message: "测试对象存储暂不可用".into(),
                });
            }
            state.concurrent_cleanup.remove(&location)
        };

        if let Some(cleanup) = concurrent_cleanup {
            complete_concurrent_cleanup(cleanup).await?;
        }
        self.state().objects.remove(&location);
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        Ok(self
            .state()
            .objects
            .contains_key(&Self::location(bucket, key)))
    }
}

async fn complete_concurrent_cleanup(cleanup: ConcurrentCleanup) -> StorageResult<()> {
    let transaction = cleanup
        .database
        .begin()
        .await
        .map_err(test_storage_database_error)?;
    sys_file::Entity::update_many()
        .col_expr(
            sys_file::Column::DelFlag,
            Expr::value(sys_file::Model::DEL_FLAG_DELETED),
        )
        .filter(sys_file::Column::Id.eq(cleanup.file_id))
        .exec(&transaction)
        .await
        .map_err(test_storage_database_error)?;
    export_job::Entity::update_many()
        .col_expr(
            export_job::Column::Status,
            Expr::value(export_job::Model::STATUS_EXPIRED),
        )
        .filter(export_job::Column::Id.eq(cleanup.export_id))
        .exec(&transaction)
        .await
        .map_err(test_storage_database_error)?;
    transaction
        .commit()
        .await
        .map_err(test_storage_database_error)
}

fn test_storage_database_error(error: sea_orm::DbErr) -> StorageError {
    StorageError::Readiness(format!("测试并发清理数据库操作失败: {error}"))
}

async fn setup_cleanup_service() -> (
    common::TestDatabase,
    Arc<ControlledObjectStorage>,
    ExportService,
) {
    let database = common::setup_test_db().await;
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(export_job::Entity))
        .await
        .unwrap();

    let cluster = DatabaseCluster::single(database.connection().clone());
    let users = Arc::new(UserService::new(
        cluster.clone(),
        AuthorizationCache::disabled(),
    ));
    let storage = Arc::new(ControlledObjectStorage::default());
    let service = ExportService::new(cluster, users, storage.clone(), &JobConfig::default());
    (database, storage, service)
}

async fn seed_expired_exports(
    database: &DatabaseConnection,
    storage: &ControlledObjectStorage,
    count: i64,
) -> Vec<SeededExport> {
    let now = Utc::now();
    let expires_at = now - Duration::hours(1);
    let completed_at = now - Duration::hours(2);
    let mut files = Vec::with_capacity(count as usize);
    let mut exports = Vec::with_capacity(count as usize);
    let mut seeded = Vec::with_capacity(count as usize);

    for index in 1..=count {
        let export_id = 10_000 + index;
        let file_id = 20_000 + index;
        let key = format!("{TENANT_ID}/exports/cleanup-{export_id}.xlsx");
        let file_name = format!("cleanup-{export_id}.xlsx");
        let payload = format!("export-{export_id}").into_bytes();
        storage.seed("exports", &key, payload.clone());
        files.push(sys_file::ActiveModel {
            id: Set(file_id),
            tenant_id: Set(TENANT_ID.into()),
            original_name: Set(file_name.clone()),
            storage_name: Set(file_name.clone()),
            storage_path: Set(key.clone()),
            bucket: Set("exports".into()),
            file_url: Set(format!("exports/{key}")),
            file_size: Set(payload.len() as i64),
            content_type: Set(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            ),
            file_sha256: Set("0".repeat(64)),
            upload_by: Set(Some("cleanup-test".into())),
            upload_status: Set(sys_file::Model::UPLOAD_STATUS_READY.into()),
            reservation_token: Set(None),
            reservation_expires_at: Set(None),
            del_flag: Set(sys_file::Model::DEL_FLAG_NORMAL.into()),
            created_at: Set(completed_at),
            updated_at: Set(completed_at),
        });
        exports.push(export_job::ActiveModel {
            id: Set(export_id),
            tenant_id: Set(TENANT_ID.into()),
            requester_id: Set(1),
            resource: Set("users".into()),
            background_job_id: Set(30_000 + index),
            request_params: Set(serde_json::json!({ "request": {} })),
            permission_code: Set("system:user:export".into()),
            status: Set(export_job::Model::STATUS_SUCCEEDED.into()),
            result_file_id: Set(Some(file_id)),
            result_file_name: Set(Some(file_name)),
            content_type: Set(Some(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            )),
            file_size: Set(Some(payload.len() as i64)),
            expires_at: Set(Some(expires_at)),
            error_message: Set(None),
            created_at: Set(completed_at),
            updated_at: Set(completed_at),
            completed_at: Set(Some(completed_at)),
        });
        seeded.push(SeededExport {
            export_id,
            file_id,
            key,
        });
    }

    sys_file::Entity::insert_many(files)
        .exec(database)
        .await
        .unwrap();
    export_job::Entity::insert_many(exports)
        .exec(database)
        .await
        .unwrap();
    seeded
}

/// 首个对象删除失败时仍须跨过批次边界清理其余任务，失败项由下一轮恢复。
#[tokio::test]
async fn cleanup_drains_all_batches_and_retries_only_the_failed_object() {
    let (database, storage, service) = setup_cleanup_service().await;
    let seeded = seed_expired_exports(database.connection(), &storage, EXPORT_COUNT).await;
    storage.fail_delete_once("exports", &seeded[0].key);

    let error = service.cleanup_expired().await.unwrap_err();
    assert!(matches!(error, AppError::ServiceUnavailable(_)));

    let exports = export_job::Entity::find()
        .order_by_asc(export_job::Column::Id)
        .all(database.connection())
        .await
        .unwrap();
    assert_eq!(exports.len(), EXPORT_COUNT as usize);
    assert_eq!(exports[0].status, export_job::Model::STATUS_SUCCEEDED);
    assert!(
        exports[1..]
            .iter()
            .all(|export| export.status == export_job::Model::STATUS_EXPIRED)
    );
    assert_eq!(
        exports.last().map(|export| export.id),
        Some(seeded.last().unwrap().export_id),
        "超过首批 100 条的任务也必须被清理"
    );
    assert_eq!(storage.object_count(), 1);

    let first_file = sys_file::Entity::find_by_id(seeded[0].file_id)
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    let last_file = sys_file::Entity::find_by_id(seeded.last().unwrap().file_id)
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_file.del_flag, sys_file::Model::DEL_FLAG_NORMAL);
    assert_eq!(last_file.del_flag, sys_file::Model::DEL_FLAG_DELETED);

    assert_eq!(service.cleanup_expired().await.unwrap(), 1);
    let exports = export_job::Entity::find()
        .all(database.connection())
        .await
        .unwrap();
    assert!(
        exports
            .iter()
            .all(|export| export.status == export_job::Model::STATUS_EXPIRED)
    );
    assert_eq!(storage.object_count(), 0);
}

/// 另一清理者抢先完成状态转换时，当前清理轮次不得重复计数或报错。
#[tokio::test]
async fn concurrent_cleanup_completion_is_not_counted_twice() {
    let (database, storage, service) = setup_cleanup_service().await;
    let seeded = seed_expired_exports(database.connection(), &storage, 1).await;
    let target = &seeded[0];
    storage.complete_concurrently_on_delete(
        "exports",
        &target.key,
        database.connection().clone(),
        target.export_id,
        target.file_id,
    );

    assert_eq!(service.cleanup_expired().await.unwrap(), 0);
    let export = export_job::Entity::find_by_id(target.export_id)
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    let file = sys_file::Entity::find_by_id(target.file_id)
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(export.status, export_job::Model::STATUS_EXPIRED);
    assert_eq!(file.del_flag, sys_file::Model::DEL_FLAG_DELETED);
    assert_eq!(storage.object_count(), 0);
}

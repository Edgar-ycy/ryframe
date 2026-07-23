mod common;

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ryframe_common::{ActorContext, DataScope, utils::file_upload::UploadConfig};
use ryframe_db::DatabaseCluster;
use ryframe_service::system::{FileService, UploadCommand};
use ryframe_storage::{LocalObjectStorage, ObjectStorage, StorageError, StorageResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
    TransactionTrait,
};
use tempfile::TempDir;
use tokio::sync::{Mutex, Semaphore};

type ObjectLocation = (String, String);
type ObjectMap = HashMap<ObjectLocation, Vec<u8>>;
type SharedObjectMap = Arc<Mutex<ObjectMap>>;

struct PausingObjectStorage {
    objects: Mutex<ObjectMap>,
    put_started: Semaphore,
    release_put: Semaphore,
    fail_after_put: bool,
}

impl PausingObjectStorage {
    fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
            put_started: Semaphore::new(0),
            release_put: Semaphore::new(0),
            fail_after_put: false,
        }
    }

    fn failing_after_put() -> Self {
        Self {
            fail_after_put: true,
            ..Self::new()
        }
    }

    async fn wait_for_put(&self) {
        self.put_started.acquire().await.unwrap().forget();
    }

    fn release_one_put(&self) {
        self.release_put.add_permits(1);
    }

    async fn object_count(&self) -> usize {
        self.objects.lock().await.len()
    }
}

#[async_trait]
impl ObjectStorage for PausingObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        self.objects
            .lock()
            .await
            .insert((bucket.to_owned(), key.to_owned()), data.to_vec());
        self.put_started.add_permits(1);
        self.release_put
            .acquire()
            .await
            .map_err(|_| StorageError::Readiness("test PUT release semaphore was closed".into()))?
            .forget();
        if self.fail_after_put {
            Err(StorageError::Service {
                operation: "test PUT",
                status: 500,
                message: "response lost after object commit".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        self.objects
            .lock()
            .await
            .get(&(bucket.to_owned(), key.to_owned()))
            .cloned()
            .ok_or_else(|| StorageError::Io {
                operation: "read test object",
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        self.objects
            .lock()
            .await
            .remove(&(bucket.to_owned(), key.to_owned()));
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        Ok(self
            .objects
            .lock()
            .await
            .contains_key(&(bucket.to_owned(), key.to_owned())))
    }
}

struct LateCompletingObjectStorage {
    objects: SharedObjectMap,
    put_started: Semaphore,
    delete_observed: Arc<Semaphore>,
}

impl LateCompletingObjectStorage {
    fn new() -> Self {
        Self {
            objects: Arc::new(Mutex::new(HashMap::new())),
            put_started: Semaphore::new(0),
            delete_observed: Arc::new(Semaphore::new(0)),
        }
    }

    async fn wait_for_put(&self) {
        self.put_started.acquire().await.unwrap().forget();
    }

    async fn object_count(&self) -> usize {
        self.objects.lock().await.len()
    }
}

struct CommitAfterCancellation {
    objects: SharedObjectMap,
    delete_observed: Arc<Semaphore>,
    location: ObjectLocation,
    data: Vec<u8>,
}

impl Drop for CommitAfterCancellation {
    fn drop(&mut self) {
        let objects = self.objects.clone();
        let delete_observed = self.delete_observed.clone();
        let location = self.location.clone();
        let data = std::mem::take(&mut self.data);
        std::mem::drop(tokio::spawn(async move {
            // Model a remote server committing only after the cancelled
            // client's first compensation DELETE has already completed.
            let Ok(permit) = delete_observed.acquire().await else {
                return;
            };
            permit.forget();
            objects.lock().await.insert(location, data);
        }));
    }
}

#[async_trait]
impl ObjectStorage for LateCompletingObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        let late_commit = CommitAfterCancellation {
            objects: self.objects.clone(),
            delete_observed: self.delete_observed.clone(),
            location: (bucket.to_owned(), key.to_owned()),
            data: data.to_vec(),
        };
        self.put_started.add_permits(1);
        std::future::pending::<()>().await;
        std::mem::forget(late_commit);
        Ok(())
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        self.objects
            .lock()
            .await
            .get(&(bucket.to_owned(), key.to_owned()))
            .cloned()
            .ok_or_else(|| StorageError::Io {
                operation: "read late test object",
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        self.objects
            .lock()
            .await
            .remove(&(bucket.to_owned(), key.to_owned()));
        self.delete_observed.add_permits(1);
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        Ok(self
            .objects
            .lock()
            .await
            .contains_key(&(bucket.to_owned(), key.to_owned())))
    }
}

struct AlwaysFailDeleteStorage {
    delete_attempts: AtomicUsize,
}

impl AlwaysFailDeleteStorage {
    fn new() -> Self {
        Self {
            delete_attempts: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ObjectStorage for AlwaysFailDeleteStorage {
    async fn put(
        &self,
        _bucket: &str,
        _key: &str,
        _data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        Err(StorageError::Readiness(
            "failing-delete storage does not accept PUT".into(),
        ))
    }

    async fn get(&self, _bucket: &str, _key: &str) -> StorageResult<Vec<u8>> {
        Err(StorageError::Io {
            operation: "read failing-delete test object",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })
    }

    async fn delete(&self, _bucket: &str, _key: &str) -> StorageResult<()> {
        self.delete_attempts.fetch_add(1, Ordering::Relaxed);
        Err(StorageError::Service {
            operation: "delete test object",
            status: 503,
            message: "forced cleanup failure".into(),
        })
    }

    async fn exists(&self, _bucket: &str, _key: &str) -> StorageResult<bool> {
        Ok(false)
    }
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "system".to_owned(),
        username: "admin".to_owned(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn count_files(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .map(|path| if path.is_dir() { count_files(&path) } else { 1 })
        .sum()
}

fn pending_file(
    id: i64,
    storage_name: &str,
    token: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> ryframe_db::entities::sys_file::Model {
    let now = chrono::Utc::now();
    let storage_path = format!("system/{storage_name}");
    ryframe_db::entities::sys_file::Model {
        id,
        tenant_id: "system".to_owned(),
        original_name: storage_name.to_owned(),
        storage_name: storage_name.to_owned(),
        storage_path: storage_path.clone(),
        bucket: "uploads".to_owned(),
        file_url: format!("uploads/{storage_path}"),
        file_size: 1,
        content_type: "text/plain".to_owned(),
        file_md5: Some(format!("{id:032x}")),
        upload_by: Some("admin".to_owned()),
        upload_status: ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_PENDING.to_owned(),
        reservation_token: Some(token.to_owned()),
        reservation_expires_at: Some(expires_at),
        del_flag: ryframe_db::entities::sys_file::Model::DEL_FLAG_UPLOAD_RESERVED.to_owned(),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn upload_persists_metadata_and_can_be_downloaded() {
    let db = common::setup_test_db().await;
    let directory = TempDir::new().unwrap();
    let service = FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        Arc::new(LocalObjectStorage::new(directory.path())),
    );
    let config = UploadConfig::default();

    let uploaded = service
        .upload_single(
            &actor(),
            UploadCommand {
                original_name: "example.txt".to_owned(),
                data: b"hello storage".to_vec(),
                config: &config,
                bucket: "uploads",
                compress: false,
            },
        )
        .await
        .unwrap();

    assert!(
        uploaded
            .file_url
            .starts_with("/api/v1/common/file/download?")
    );
    assert_eq!(count_files(directory.path()), 1);
    let (data, filename) = service
        .download(&actor(), "uploads", &uploaded.file_info.file_path)
        .await
        .unwrap();
    assert_eq!(data, b"hello storage");
    assert_eq!(filename, uploaded.file_info.storage_name);
}

#[tokio::test]
async fn metadata_failure_deletes_the_uploaded_object() {
    let db = common::setup_test_db().await;
    db.execute_unprepared(
        "CREATE TRIGGER reject_file_metadata BEFORE INSERT ON sys_file \
         FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'forced metadata failure'",
    )
    .await
    .unwrap();
    let directory = TempDir::new().unwrap();
    let service = FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        Arc::new(LocalObjectStorage::new(directory.path())),
    );
    let config = UploadConfig::default();

    let result = service
        .upload_single(
            &actor(),
            UploadCommand {
                original_name: "example.txt".to_owned(),
                data: b"must be cleaned".to_vec(),
                config: &config,
                bucket: "uploads",
                compress: false,
            },
        )
        .await;

    assert!(result.is_err());
    assert_eq!(count_files(directory.path()), 0);
}

#[tokio::test]
async fn concurrent_uploads_cannot_exceed_tenant_storage_quota() {
    let db = common::setup_test_db().await;
    let tenant = ryframe_db::entities::tenant::Entity::find()
        .filter(ryframe_db::entities::tenant::Column::TenantId.eq("system"))
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    let mut active: ryframe_db::entities::tenant::ActiveModel = tenant.into();
    active.max_storage_mb = Set(1);
    active.update(db.connection()).await.unwrap();

    let directory = TempDir::new().unwrap();
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        Arc::new(LocalObjectStorage::new(directory.path())),
    ));
    let config = UploadConfig::default();
    let actor_a = actor();
    let actor_b = actor();

    let upload_a = service.upload_single(
        &actor_a,
        UploadCommand {
            original_name: "quota-a.txt".to_owned(),
            data: vec![b'a'; 700 * 1024],
            config: &config,
            bucket: "uploads",
            compress: false,
        },
    );
    let upload_b = service.upload_single(
        &actor_b,
        UploadCommand {
            original_name: "quota-b.txt".to_owned(),
            data: vec![b'b'; 700 * 1024],
            config: &config,
            bucket: "uploads",
            compress: false,
        },
    );

    let (result_a, result_b) = tokio::join!(upload_a, upload_b);
    let results = [result_a, result_b];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().any(|result| {
        matches!(result, Err(ryframe_common::AppError::Validation(message)) if message.contains("存储容量"))
    }));

    let metadata_count = ryframe_db::entities::sys_file::Entity::find()
        .filter(ryframe_db::entities::sys_file::Column::TenantId.eq("system"))
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(metadata_count, 1);
    assert_eq!(count_files(directory.path()), 1);
}

#[tokio::test]
async fn object_put_does_not_hold_the_tenant_row_lock() {
    let db = common::setup_test_db().await;
    let storage = Arc::new(PausingObjectStorage::new());
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    ));
    let upload_service = service.clone();
    let upload = tokio::spawn(async move {
        let config = UploadConfig::default();
        upload_service
            .upload_single(
                &actor(),
                UploadCommand {
                    original_name: "slow.txt".to_owned(),
                    data: b"slow object body".to_vec(),
                    config: &config,
                    bucket: "uploads",
                    compress: false,
                },
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(3), storage.wait_for_put())
        .await
        .expect("upload never reached object storage");

    // A previous-version reader only knows `del_flag = '0'`; rolling
    // deployments must never expose a pending reservation as a normal file.
    let legacy_visible = ryframe_db::entities::sys_file::Entity::find()
        .filter(
            ryframe_db::entities::sys_file::Column::DelFlag
                .eq(ryframe_db::entities::sys_file::Model::DEL_FLAG_NORMAL),
        )
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(legacy_visible, 0);
    let reserved = ryframe_db::entities::sys_file::Entity::find()
        .filter(
            ryframe_db::entities::sys_file::Column::DelFlag
                .eq(ryframe_db::entities::sys_file::Model::DEL_FLAG_UPLOAD_RESERVED),
        )
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(reserved, 1);

    let transaction = db.connection().begin().await.unwrap();
    let lock_result = tokio::time::timeout(
        Duration::from_secs(1),
        ryframe_db::TenantRepository.lock_tenant_in_txn(&transaction, "system"),
    )
    .await;
    storage.release_one_put();
    assert!(
        matches!(lock_result, Ok(Ok(_))),
        "tenant row remained locked while object PUT was pending: {lock_result:?}"
    );
    transaction.rollback().await.unwrap();
    upload.await.unwrap().unwrap();
    let legacy_visible = ryframe_db::entities::sys_file::Entity::find()
        .filter(
            ryframe_db::entities::sys_file::Column::DelFlag
                .eq(ryframe_db::entities::sys_file::Model::DEL_FLAG_NORMAL),
        )
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(legacy_visible, 1);
}

#[tokio::test]
async fn pending_upload_reservation_counts_against_tenant_quota() {
    let db = common::setup_test_db().await;
    let tenant = ryframe_db::entities::tenant::Entity::find()
        .filter(ryframe_db::entities::tenant::Column::TenantId.eq("system"))
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    let mut active: ryframe_db::entities::tenant::ActiveModel = tenant.into();
    active.max_storage_mb = Set(1);
    active.update(db.connection()).await.unwrap();

    let storage = Arc::new(PausingObjectStorage::new());
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    ));
    let first_service = service.clone();
    let first = tokio::spawn(async move {
        let config = UploadConfig::default();
        first_service
            .upload_single(
                &actor(),
                UploadCommand {
                    original_name: "reserved-a.txt".to_owned(),
                    data: vec![b'a'; 700 * 1024],
                    config: &config,
                    bucket: "uploads",
                    compress: false,
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), storage.wait_for_put())
        .await
        .expect("first upload never reached object storage");

    let config = UploadConfig::default();
    let second = tokio::time::timeout(
        Duration::from_secs(3),
        service.upload_single(
            &actor(),
            UploadCommand {
                original_name: "reserved-b.txt".to_owned(),
                data: vec![b'b'; 700 * 1024],
                config: &config,
                bucket: "uploads",
                compress: false,
            },
        ),
    )
    .await
    .expect("quota check blocked behind object PUT");
    assert!(
        matches!(second, Err(ryframe_common::AppError::Validation(message)) if message.contains("存储容量"))
    );

    storage.release_one_put();
    first.await.unwrap().unwrap();
    assert_eq!(storage.object_count().await, 1);
}

#[tokio::test]
async fn cancelling_put_persists_cleanup_state_and_deletes_the_object() {
    let db = common::setup_test_db().await;
    let storage = Arc::new(PausingObjectStorage::new());
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    ));
    let upload_service = service.clone();
    let upload = tokio::spawn(async move {
        let config = UploadConfig::default();
        upload_service
            .upload_single(
                &actor(),
                UploadCommand {
                    original_name: "cancelled.txt".to_owned(),
                    data: b"cancelled object body".to_vec(),
                    config: &config,
                    bucket: "uploads",
                    compress: false,
                },
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(3), storage.wait_for_put())
        .await
        .expect("upload never reached object storage");
    assert_eq!(storage.object_count().await, 1);
    upload.abort();
    assert!(upload.await.unwrap_err().is_cancelled());

    let cleanup = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let row = ryframe_db::entities::sys_file::Entity::find()
                .filter(ryframe_db::entities::sys_file::Column::TenantId.eq("system"))
                .one(db.connection())
                .await
                .unwrap();
            if let Some(row) = row
                && row.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
                && storage.object_count().await == 0
            {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable upload compensation did not finish");

    assert!(
        cleanup
            .reservation_expires_at
            .is_some_and(|expires_at| expires_at > chrono::Utc::now())
    );
    assert_eq!(
        cleanup.del_flag,
        ryframe_db::entities::sys_file::Model::DEL_FLAG_UPLOAD_RESERVED
    );
    let legacy_visible = ryframe_db::entities::sys_file::Entity::find()
        .filter(
            ryframe_db::entities::sys_file::Column::DelFlag
                .eq(ryframe_db::entities::sys_file::Model::DEL_FLAG_NORMAL),
        )
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(legacy_visible, 0);
}

#[tokio::test]
async fn object_committed_before_put_error_is_compensated() {
    let db = common::setup_test_db().await;
    let storage = Arc::new(PausingObjectStorage::failing_after_put());
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    ));
    let upload_service = service.clone();
    let upload = tokio::spawn(async move {
        let config = UploadConfig::default();
        upload_service
            .upload_single(
                &actor(),
                UploadCommand {
                    original_name: "ambiguous-put.txt".to_owned(),
                    data: b"object was actually committed".to_vec(),
                    config: &config,
                    bucket: "uploads",
                    compress: false,
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), storage.wait_for_put())
        .await
        .expect("upload never reached object storage");
    assert_eq!(storage.object_count().await, 1);
    storage.release_one_put();

    assert!(matches!(
        upload.await.unwrap(),
        Err(ryframe_common::AppError::ServiceUnavailable(_))
    ));
    assert_eq!(storage.object_count().await, 0);
    let row = ryframe_db::entities::sys_file::Entity::find()
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.upload_status,
        ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
    );
}

#[tokio::test]
async fn janitor_deletes_an_object_that_commits_after_cancellation_cleanup() {
    let db = common::setup_test_db().await;
    let storage = Arc::new(LateCompletingObjectStorage::new());
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    ));
    let upload_service = service.clone();
    let upload = tokio::spawn(async move {
        let config = UploadConfig::default();
        upload_service
            .upload_single(
                &actor(),
                UploadCommand {
                    original_name: "late-commit.txt".to_owned(),
                    data: b"committed after cancellation".to_vec(),
                    config: &config,
                    bucket: "uploads",
                    compress: false,
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), storage.wait_for_put())
        .await
        .expect("upload never reached object storage");
    upload.abort();
    assert!(upload.await.unwrap_err().is_cancelled());

    let cleanup = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let row = ryframe_db::entities::sys_file::Entity::find()
                .one(db.connection())
                .await
                .unwrap();
            if let Some(row) = row
                && row.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
                && storage.object_count().await == 1
            {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("late object commit was not observed");

    let mut active: ryframe_db::entities::sys_file::ActiveModel = cleanup.into();
    active.reservation_expires_at = Set(Some(chrono::Utc::now() - chrono::Duration::seconds(1)));
    active.update(db.connection()).await.unwrap();
    assert_eq!(service.reconcile_upload_reservations().await.unwrap(), 1);
    assert_eq!(storage.object_count().await, 0);
    assert!(
        ryframe_db::entities::sys_file::Entity::find()
            .one(db.connection())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn ready_state_write_failure_is_compensated() {
    let db = common::setup_test_db().await;
    db.execute_unprepared(
        "CREATE TRIGGER reject_file_ready BEFORE UPDATE ON sys_file \
         FOR EACH ROW BEGIN \
           IF NEW.upload_status = 'ready' THEN \
             SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'forced ready failure'; \
           END IF; \
         END",
    )
    .await
    .unwrap();
    let directory = TempDir::new().unwrap();
    let service = FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        Arc::new(LocalObjectStorage::new(directory.path())),
    );
    let config = UploadConfig::default();

    let result = service
        .upload_single(
            &actor(),
            UploadCommand {
                original_name: "ready-failure.txt".to_owned(),
                data: b"must be compensated".to_vec(),
                config: &config,
                bucket: "uploads",
                compress: false,
            },
        )
        .await;

    assert!(matches!(result, Err(ryframe_common::AppError::Database(_))));
    assert_eq!(count_files(directory.path()), 0);
    let row = ryframe_db::entities::sys_file::Entity::find()
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.upload_status,
        ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
    );
}

#[tokio::test]
async fn concurrent_duplicate_uploads_share_one_durable_object() {
    let db = common::setup_test_db().await;
    let directory = TempDir::new().unwrap();
    let service = Arc::new(FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        Arc::new(LocalObjectStorage::new(directory.path())),
    ));
    let config = UploadConfig::default();
    let actor_a = actor();
    let actor_b = actor();
    let body = b"same content".to_vec();

    let upload_a = service.upload_single(
        &actor_a,
        UploadCommand {
            original_name: "duplicate-a.txt".to_owned(),
            data: body.clone(),
            config: &config,
            bucket: "uploads",
            compress: false,
        },
    );
    let upload_b = service.upload_single(
        &actor_b,
        UploadCommand {
            original_name: "duplicate-b.txt".to_owned(),
            data: body.clone(),
            config: &config,
            bucket: "uploads",
            compress: false,
        },
    );
    let (result_a, result_b) = tokio::join!(upload_a, upload_b);
    assert!(result_a.is_ok() || result_b.is_ok());

    let retry = service
        .upload_single(
            &actor(),
            UploadCommand {
                original_name: "duplicate-retry.txt".to_owned(),
                data: body,
                config: &config,
                bucket: "uploads",
                compress: false,
            },
        )
        .await
        .unwrap();
    for successful in [result_a.as_ref().ok(), result_b.as_ref().ok()]
        .into_iter()
        .flatten()
    {
        assert_eq!(successful.file_id, retry.file_id);
    }

    let metadata_count = ryframe_db::entities::sys_file::Entity::find()
        .filter(ryframe_db::entities::sys_file::Column::TenantId.eq("system"))
        .count(db.connection())
        .await
        .unwrap();
    assert_eq!(metadata_count, 1);
    assert_eq!(count_files(directory.path()), 1);
}

#[tokio::test]
async fn expired_pending_upload_uses_a_cleanup_grace_before_hard_delete() {
    let db = common::setup_test_db().await;
    let directory = TempDir::new().unwrap();
    let storage = Arc::new(LocalObjectStorage::new(directory.path()));
    let stale_id =
        ryframe_common::utils::snowflake::try_next_snowflake_id().expect("generate test ID");
    let stale_path = "system/stale-upload.txt";
    let stale_body = b"late object";
    storage
        .put("uploads", stale_path, stale_body, "text/plain")
        .await
        .unwrap();
    let now = chrono::Utc::now();
    let stale = ryframe_db::entities::sys_file::Model {
        id: stale_id,
        tenant_id: "system".to_owned(),
        original_name: "stale.txt".to_owned(),
        storage_name: "stale-upload.txt".to_owned(),
        storage_path: stale_path.to_owned(),
        bucket: "uploads".to_owned(),
        file_url: format!("uploads/{stale_path}"),
        file_size: stale_body.len() as i64,
        content_type: "text/plain".to_owned(),
        file_md5: Some(format!("{:x}", md5::compute(stale_body))),
        upload_by: Some("admin".to_owned()),
        upload_status: ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_PENDING.to_owned(),
        reservation_token: Some("stale-token".to_owned()),
        reservation_expires_at: Some(now - chrono::Duration::minutes(1)),
        del_flag: ryframe_db::entities::sys_file::Model::DEL_FLAG_UPLOAD_RESERVED.to_owned(),
        created_at: now,
        updated_at: now,
    };
    let active: ryframe_db::entities::sys_file::ActiveModel = stale.into();
    ryframe_db::entities::sys_file::Entity::insert(active)
        .exec(db.connection())
        .await
        .unwrap();

    let service = FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    );
    assert_eq!(service.reconcile_upload_reservations().await.unwrap(), 1);

    let cleanup = ryframe_db::entities::sys_file::Entity::find_by_id(stale_id)
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cleanup.upload_status,
        ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
    );
    assert!(storage.exists("uploads", stale_path).await.unwrap());

    let mut active: ryframe_db::entities::sys_file::ActiveModel = cleanup.into();
    active.reservation_expires_at = Set(Some(chrono::Utc::now() - chrono::Duration::seconds(1)));
    active.update(db.connection()).await.unwrap();
    assert_eq!(service.reconcile_upload_reservations().await.unwrap(), 1);

    assert!(
        ryframe_db::entities::sys_file::Entity::find_by_id(stale_id)
            .one(db.connection())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!storage.exists("uploads", stale_path).await.unwrap());
}

#[tokio::test]
async fn upload_state_compare_and_set_races_have_exactly_one_winner() {
    let db = common::setup_test_db().await;
    let database_now = ryframe_db::FileRepository
        .database_utc_now(db.connection())
        .await
        .unwrap();

    let finalize_id =
        ryframe_common::utils::snowflake::try_next_snowflake_id().expect("generate test ID");
    let finalize = pending_file(
        finalize_id,
        "finalize-race.txt",
        "finalize-token",
        database_now + chrono::Duration::minutes(5),
    );
    let active: ryframe_db::entities::sys_file::ActiveModel = finalize.into();
    ryframe_db::entities::sys_file::Entity::insert(active)
        .exec(db.connection())
        .await
        .unwrap();
    let mark_db = db.connection().clone();
    let cleanup_db = db.connection().clone();
    let (marked_ready, began_cleanup) = tokio::join!(
        ryframe_db::FileRepository.mark_ready(
            &mark_db,
            "system",
            finalize_id,
            "finalize-token",
            database_now,
        ),
        ryframe_db::FileRepository.begin_cleanup(
            &cleanup_db,
            "system",
            finalize_id,
            "finalize-token",
            database_now + chrono::Duration::minutes(5),
        ),
    );
    assert_ne!(marked_ready.unwrap(), began_cleanup.unwrap());

    let lease_id =
        ryframe_common::utils::snowflake::try_next_snowflake_id().expect("generate test ID");
    let lease = pending_file(
        lease_id,
        "lease-race.txt",
        "lease-token",
        database_now - chrono::Duration::seconds(1),
    );
    let active: ryframe_db::entities::sys_file::ActiveModel = lease.into();
    ryframe_db::entities::sys_file::Entity::insert(active)
        .exec(db.connection())
        .await
        .unwrap();
    let renew_db = db.connection().clone();
    let expiry_db = db.connection().clone();
    let (renewed, expired_cleanup) = tokio::join!(
        ryframe_db::FileRepository.renew_pending_reservation(
            &renew_db,
            "system",
            lease_id,
            "lease-token",
            database_now + chrono::Duration::minutes(5),
        ),
        ryframe_db::FileRepository.begin_expired_cleanup(
            &expiry_db,
            "system",
            lease_id,
            database_now,
            database_now + chrono::Duration::minutes(5),
        ),
    );
    assert_ne!(renewed.unwrap(), expired_cleanup.unwrap());

    let cleanup_id =
        ryframe_common::utils::snowflake::try_next_snowflake_id().expect("generate test ID");
    let mut cleanup = pending_file(
        cleanup_id,
        "cleanup-extension.txt",
        "cleanup-token",
        database_now,
    );
    cleanup.upload_status = ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP.to_owned();
    let active: ryframe_db::entities::sys_file::ActiveModel = cleanup.into();
    ryframe_db::entities::sys_file::Entity::insert(active)
        .exec(db.connection())
        .await
        .unwrap();
    let extended_until = database_now + chrono::Duration::minutes(10);
    assert!(
        ryframe_db::FileRepository
            .begin_cleanup(
                db.connection(),
                "system",
                cleanup_id,
                "cleanup-token",
                extended_until,
            )
            .await
            .unwrap()
    );
    let cleanup = ryframe_db::entities::sys_file::Entity::find_by_id(cleanup_id)
        .one(db.connection())
        .await
        .unwrap()
        .unwrap();
    let stored_expiry = cleanup.reservation_expires_at.unwrap();
    assert!((stored_expiry - extended_until).num_seconds().abs() <= 1);
    assert!(stored_expiry > database_now + chrono::Duration::minutes(9));
}

#[tokio::test]
async fn failed_cleanup_batch_is_deferred_so_later_rows_are_not_starved() {
    let db = common::setup_test_db().await;
    let database_now = ryframe_db::FileRepository
        .database_utc_now(db.connection())
        .await
        .unwrap();
    for index in 0..33 {
        let id =
            ryframe_common::utils::snowflake::try_next_snowflake_id().expect("generate test ID");
        let mut cleanup = pending_file(
            id,
            &format!("failed-cleanup-{index}.txt"),
            &format!("cleanup-token-{index}"),
            database_now - chrono::Duration::seconds(1),
        );
        cleanup.upload_status =
            ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP.to_owned();
        let active: ryframe_db::entities::sys_file::ActiveModel = cleanup.into();
        ryframe_db::entities::sys_file::Entity::insert(active)
            .exec(db.connection())
            .await
            .unwrap();
    }

    let storage = Arc::new(AlwaysFailDeleteStorage::new());
    let service = FileService::new(
        DatabaseCluster::single(db.connection().clone()),
        storage.clone(),
    );
    assert_eq!(service.reconcile_upload_reservations().await.unwrap(), 0);
    assert_eq!(storage.delete_attempts.load(Ordering::Relaxed), 32);

    // The first 32 failures were moved into the future, so the next bounded
    // scan reaches the 33rd row instead of retrying the same hot set forever.
    assert_eq!(service.reconcile_upload_reservations().await.unwrap(), 0);
    assert_eq!(storage.delete_attempts.load(Ordering::Relaxed), 33);
    let current_database_time = ryframe_db::FileRepository
        .database_utc_now(db.connection())
        .await
        .unwrap();
    let rows = ryframe_db::entities::sys_file::Entity::find()
        .all(db.connection())
        .await
        .unwrap();
    assert_eq!(rows.len(), 33);
    assert!(rows.into_iter().all(|row| {
        row.reservation_expires_at
            .is_some_and(|retry_at| retry_at > current_database_time)
    }));
}

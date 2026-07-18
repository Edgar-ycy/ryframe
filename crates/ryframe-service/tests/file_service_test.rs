mod common;

use std::{path::Path, sync::Arc};

use ryframe_common::{ActorContext, DataScope, utils::file_upload::UploadConfig};
use ryframe_db::DatabaseCluster;
use ryframe_service::system::{FileService, UploadCommand};
use ryframe_storage::LocalObjectStorage;
use sea_orm::ConnectionTrait;
use tempfile::TempDir;

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

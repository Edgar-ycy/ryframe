use std::sync::Arc;

use crate::{ControlDatabaseCluster, FileRepository, ReadConsistency};

use ryframe_application::{
    PersistenceFuture,
    ports::files::{FileDownloadPersistencePort, FileDownloadRecord},
};

struct DatabaseFileDownloadPersistence {
    database: ControlDatabaseCluster,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn FileDownloadPersistencePort> {
    Arc::new(DatabaseFileDownloadPersistence { database })
}

impl FileDownloadPersistencePort for DatabaseFileDownloadPersistence {
    fn find_by_storage_path<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        storage_path: &'a str,
    ) -> PersistenceFuture<'a, Option<FileDownloadRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            FileRepository
                .find_by_storage_path(&database, tenant_id, bucket, storage_path)
                .await
                .map(|record| record.map(map_record))
        })
    }

    fn find_ready_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        expected_bucket: &'a str,
    ) -> PersistenceFuture<'a, Option<FileDownloadRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            FileRepository
                .find_by_id_any_status(&database, tenant_id, file_id)
                .await
                .map(|record| {
                    record
                        .filter(|file| {
                            file.bucket == expected_bucket
                                && file.upload_status
                                    == crate::entities::sys_file::Model::UPLOAD_STATUS_READY
                        })
                        .map(map_record)
                })
        })
    }
}

fn map_record(file: crate::entities::sys_file::Model) -> FileDownloadRecord {
    FileDownloadRecord {
        bucket: file.bucket,
        storage_path: file.storage_path,
        original_name: file.original_name,
        content_type: file.content_type,
    }
}

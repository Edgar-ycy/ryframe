use std::time::{Duration as StdDuration, Instant};

use ryframe_application::{
    ports::{export::*, files::*},
    system::export::*,
};
use ryframe_kernel::*;

mod purge {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ryframe_application::ports::files::{
        ArtifactStoreError, ArtifactStoreErrorKind, ArtifactStoreFuture,
    };

    use super::*;

    struct RetryStorage {
        attempts: AtomicUsize,
    }

    impl ArtifactStore for RetryStorage {
        fn readiness<'a>(&'a self, _bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不检查存储")
        }

        fn ensure_bucket<'a>(&'a self, _bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不创建桶")
        }

        fn put<'a>(
            &'a self,
            _bucket: &'a str,
            _key: &'a str,
            _data: &'a [u8],
            _content_type: &'a str,
        ) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不写对象")
        }

        fn put_file<'a>(
            &'a self,
            _bucket: &'a str,
            _key: &'a str,
            _path: &'a std::path::Path,
            _content_type: &'a str,
            _sha256_hex: Option<&'a str>,
        ) -> ArtifactStoreFuture<'a, ()> {
            unreachable!("测试不写对象")
        }

        fn get<'a>(&'a self, _bucket: &'a str, _key: &'a str) -> ArtifactStoreFuture<'a, Vec<u8>> {
            unreachable!("测试不读对象")
        }

        fn delete<'a>(&'a self, _bucket: &'a str, _key: &'a str) -> ArtifactStoreFuture<'a, ()> {
            Box::pin(async move {
                if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(ArtifactStoreError::new(
                        ArtifactStoreErrorKind::Unavailable,
                        "临时不可用",
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn file() -> ExportCleanupFile {
        ExportCleanupFile {
            id: 1,
            storage_path: "tenant-a/exports/users-1.xlsx".into(),
            bucket: EXPORT_BUCKET.into(),
        }
    }

    #[tokio::test]
    async fn storage_failure_keeps_cleanup_retryable() {
        let storage = RetryStorage {
            attempts: AtomicUsize::new(0),
        };
        let artifact = file();
        assert!(
            delete_object_idempotently(&storage, &artifact)
                .await
                .is_err()
        );
        delete_object_idempotently(&storage, &artifact)
            .await
            .expect("重试应能够完成幂等删除");
        assert_eq!(storage.attempts.load(Ordering::SeqCst), 2);
    }
}

mod resources {
    use super::*;

    const _: () = {
        assert!(EXPORT_BATCH_SIZE == 1_000);
        assert!(EXPORT_MAX_RUNTIME_SECONDS == 1_800);
        assert!(EXPORT_MAX_RUNNING_PER_TENANT == 2);
        assert!(EXPORT_MAX_RESULT_BYTES == 512 * 1024 * 1024);
    };

    #[test]
    fn execution_snapshot_uses_application_record() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
            .expect("测试时间应有效")
            .with_timezone(&chrono::Utc);
        let record = ExportExecutionRecord {
            id: 1,
            tenant_id: "tenant-a".into(),
            requester_id: 7,
            resource: "users".into(),
            request_params: serde_json::json!({}),
            request_version: i32::from(EXPORT_REQUEST_VERSION),
            permission_code: "system:user:export".into(),
            authorization_fingerprint: "authorization".into(),
            snapshot_at: now,
            upper_id: 99,
            matched_rows: 8,
            status: EXPORT_STATUS_RUNNING.into(),
        };

        let snapshot = export_execution_snapshot(&record);
        assert_eq!(snapshot.request_version, i32::from(EXPORT_REQUEST_VERSION));
        assert_eq!(snapshot.authorization_fingerprint, "authorization");
        assert_eq!(snapshot.snapshot_at, &now);
        assert_eq!(snapshot.upper_id, 99);
        assert_eq!(snapshot.matched_rows, 8);
    }

    #[test]
    fn cursor_window_rejects_non_advancing_or_oversized_batches() {
        let window = ExportCursorWindow::new(Some(10), 20, 2);
        assert!(last_batch_id(&["11", "20"], window, |id| id).is_ok());
        assert!(last_batch_id(&["10"], window, |id| id).is_err());
        assert!(last_batch_id(&["12", "11"], window, |id| id).is_err());
        assert!(last_batch_id(&["11", "12", "13"], window, |id| id).is_err());
    }

    #[test]
    fn row_and_byte_limits_fail_closed() {
        validate_row_and_byte_limits(500_000, 512 * 1024 * 1024, 500_000, 500_000)
            .expect("边界值应可用");
        assert!(validate_row_and_byte_limits(500_001, 1, 500_001, 500_000).is_err());
        assert!(validate_row_and_byte_limits(1, 512 * 1024 * 1024 + 1, 1, 500_000).is_err());

        let started_at = Instant::now()
            .checked_sub(StdDuration::from_secs(1_800))
            .expect("测试时间应可回退");
        assert!(validate_export_runtime(started_at, 1, 0, 0, 500_000).is_err());
    }

    #[test]
    fn deletion_after_request_may_finish_with_fewer_rows_but_new_ids_are_rejected() {
        validate_row_and_byte_limits(998, 1, 1_000, 500_000)
            .expect("执行前删除应允许实际导出少于申请时匹配数");
        let snapshot = ExportCursorWindow::new(Some(998), 1_000, 1_000);
        assert!(last_batch_id(&["999", "1000"], snapshot, |id| id).is_ok());
        assert!(last_batch_id(&["1001"], snapshot, |id| id).is_err());
    }
}

mod storage {
    use super::*;

    #[test]
    fn running_export_requires_artifact_write() {
        let current = ExportArtifactState {
            status: EXPORT_STATUS_RUNNING.into(),
            result_file_id: None,
        };
        assert!(artifact_write_required(&current, 42).expect("运行任务应继续落账"));
    }

    #[test]
    fn matching_succeeded_export_is_idempotent() {
        let current = ExportArtifactState {
            status: EXPORT_STATUS_SUCCEEDED.into(),
            result_file_id: Some(42),
        };
        assert!(!artifact_write_required(&current, 42).expect("相同文件应幂等完成"));
    }

    #[test]
    fn conflicting_or_terminal_export_is_rejected() {
        let conflicting = ExportArtifactState {
            status: EXPORT_STATUS_SUCCEEDED.into(),
            result_file_id: Some(43),
        };
        let cancelled = ExportArtifactState {
            status: EXPORT_STATUS_CANCELLED.into(),
            result_file_id: None,
        };
        assert!(matches!(
            artifact_write_required(&conflicting, 42),
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            artifact_write_required(&cancelled, 42),
            Err(AppError::Conflict(_))
        ));
    }
}

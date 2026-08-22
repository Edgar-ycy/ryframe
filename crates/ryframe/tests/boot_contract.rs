use std::io::{Cursor, Write};

use ryframe_adapters::{RefreshRotation, storage::StorageError};
use ryframe_api::session_security::SessionRevocation;
use ryframe_application::{
    CacheAvailabilityPolicy,
    ports::{
        auth::{
            RefreshSessionFamily, RefreshSessionRevocation as ApplicationSessionRevocation,
            RefreshSessionRotation,
        },
        files::ArtifactStoreErrorKind,
        spreadsheet::SpreadsheetRow,
    },
};
use ryframe_kernel::AppError;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

mod authorization_cache {
    use super::*;
    use ryframe::boot::authorization_cache::*;

    #[test]
    fn validates_cache_versions_without_lexical_ordering() {
        assert_eq!(version_is_newer(Some("10"), "9"), Ok(true));
        assert_eq!(version_is_newer(Some("9"), "10"), Ok(false));
        assert!(validate_canonical_decimal("0").is_ok());
        assert!(validate_canonical_decimal("01").is_err());
        assert!(validate_canonical_decimal("-1").is_err());
    }

    #[tokio::test]
    async fn disabled_cache_respects_availability_policy() {
        let optional = cache(None, CacheAvailabilityPolicy::Optional);
        assert!(!optional.is_enabled());
        assert!(
            optional
                .lookup_snapshot("tenant-a", 1)
                .await
                .expect("可选缓存应回退")
                .snapshot
                .is_none()
        );

        let required = cache(None, CacheAvailabilityPolicy::Required);
        assert!(required.lookup_snapshot("tenant-a", 1).await.is_err());
    }
}

mod authorization_cache_keyspace {
    use ryframe::boot::authorization_cache_keyspace::*;

    #[test]
    fn tenant_keys_share_one_cluster_hash_tag() {
        let epoch = tenant_epoch_key("tenant-a");
        let user = user_version_key("tenant-a", 42);
        let snapshot = snapshot_hash_key("tenant-a", 42);

        assert!(epoch.contains("{tenant-a}"));
        assert!(user.contains("{tenant-a}"));
        assert!(snapshot.contains("{tenant-a}"));
        assert_ne!(epoch, tenant_epoch_key("tenant-b"));
    }
}

mod login_protection {
    use ryframe::boot::login_protection::*;

    #[test]
    fn keys_normalize_principal_and_hide_raw_values() {
        assert_eq!(
            principal_key("tenant-a", " Alice "),
            principal_key("tenant-a", "alice")
        );
        assert!(!ip_key("tenant-a", "192.0.2.1").contains("192.0.2.1"));
    }

    #[tokio::test]
    async fn missing_redis_disables_login_protection_without_error() {
        let store = store(None);
        store
            .ensure_allowed("tenant-a", "alice", "192.0.2.1", 5)
            .await
            .expect("未配置 Redis 时不应阻止登录");
        store
            .record_failure("tenant-a", "alice", "192.0.2.1", 60)
            .await
            .expect("未配置 Redis 时记录失败应为空操作");
        store
            .clear("tenant-a", "alice", "192.0.2.1")
            .await
            .expect("未配置 Redis 时清理应为空操作");
    }
}

mod file_content {
    use ryframe::boot::file_content::*;

    use ryframe_kernel::AppError;

    #[tokio::test]
    async fn preserves_valid_text_without_compression() {
        let processed = processor()
            .process("note.txt".into(), b"hello".to_vec(), false)
            .await
            .expect("合法文本应完成处理");

        assert_eq!(processed.original_name, "note.txt");
        assert_eq!(processed.file_name, "note.txt");
        assert_eq!(processed.data, b"hello");
        assert_eq!(processed.content_type, "text/plain");
    }

    #[tokio::test]
    async fn rejects_content_that_does_not_match_extension() {
        let result = processor()
            .process("image.png".into(), b"not-a-png".to_vec(), false)
            .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }
}

mod artifact_store {
    use super::*;
    use ryframe::boot::artifact_store::*;

    #[test]
    fn storage_errors_are_mapped_to_stable_application_kinds() {
        let not_found = map_storage_error(StorageError::Service {
            operation: "GET",
            status: 404,
            message: "missing".into(),
        });
        let unavailable = map_storage_error(StorageError::Service {
            operation: "PUT",
            status: 503,
            message: "busy".into(),
        });
        let misconfigured = map_storage_error(StorageError::Unsupported("operation".into()));

        assert_eq!(not_found.kind(), ArtifactStoreErrorKind::NotFound);
        assert_eq!(unavailable.kind(), ArtifactStoreErrorKind::Unavailable);
        assert_eq!(misconfigured.kind(), ArtifactStoreErrorKind::Misconfigured);
    }
}

mod agent_limiter {
    use ryframe::boot::agent_limiter::*;

    #[test]
    fn digest_separates_dimensions_and_values() {
        assert_ne!(digest_key("tenant", "12"), digest_key("tenant1", "2"));
        assert_ne!(digest_key("tenant", "12"), digest_key("account", "12"));
        assert!(digest_key("tenant", "12").starts_with("ryframe:agent-limit:tenant:"));
    }
}

mod message_listener {
    use ryframe::boot::message_listener::*;

    #[test]
    fn scoped_channels_are_classified_by_their_physical_names() {
        let authorization = "ryframe:test:authorization";
        let message = "ryframe:test:message";

        assert_eq!(
            classify_channel(authorization, authorization, message),
            Some(WakeupKind::AuthorizationChanged)
        );
        assert_eq!(
            classify_channel(message, authorization, message),
            Some(WakeupKind::Message)
        );
        assert_eq!(
            classify_channel("ryframe:other", authorization, message),
            None
        );
    }
}

mod online_sessions {
    use ryframe::boot::online_sessions::*;

    #[test]
    fn session_and_indexes_have_distinct_keyspaces() {
        let session = session_key("tenant-a", "sid-a");
        let tenant = tenant_index_key("tenant-a");
        let user = tenant_user_index_key("tenant-a", 42);

        assert_ne!(session, tenant);
        assert_ne!(session, user);
        assert_ne!(tenant, user);
        assert!(session.ends_with("tenant-a:sid-a"));
    }
}

mod refresh_sessions {
    use super::*;
    use ryframe::boot::refresh_sessions::*;

    #[test]
    fn maps_all_refresh_rotation_outcomes() {
        assert_eq!(
            map_rotation(RefreshRotation::Concurrent),
            RefreshSessionRotation::Concurrent
        );
        assert_eq!(
            map_rotation(RefreshRotation::Replayed),
            RefreshSessionRotation::Replayed
        );
        assert_eq!(
            map_rotation(RefreshRotation::MissingOrRevoked),
            RefreshSessionRotation::MissingOrRevoked
        );
    }

    #[tokio::test]
    async fn memory_store_preserves_refresh_family_semantics() {
        let sessions = store(None);
        let sid = ryframe_auth::jwt::new_sid();
        let now = chrono::Utc::now().timestamp();
        sessions
            .register(RefreshSessionFamily {
                sid: sid.clone(),
                tenant_id: "tenant-a".into(),
                user_id: 42,
                current_jti: "jti-1".into(),
                previous_jti: None,
                last_attempt_id: None,
                rotated_at: now,
                absolute_exp: now + 60,
                revoked: false,
            })
            .await
            .expect("应登记刷新会话族");

        let identity = sessions
            .identity(&sid)
            .await
            .expect("应读取刷新会话")
            .expect("刷新会话应存在");
        assert_eq!(identity.tenant_id, "tenant-a");
        assert_eq!(identity.user_id, 42);
        assert!(matches!(
            sessions
                .rotate(&sid, "jti-1", "jti-2", now + 1, "attempt-1")
                .await
                .expect("应轮换刷新令牌"),
            RefreshSessionRotation::Rotated { .. }
        ));
        assert!(sessions.revoke(&sid).await.expect("应撤销刷新会话"));
        assert!(
            sessions
                .identity(&sid)
                .await
                .expect("应读取撤销状态")
                .is_none()
        );
    }
}

mod session_security {
    use super::*;
    use ryframe::boot::session_security::*;

    #[test]
    fn session_revocation_mapping_is_complete() {
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::Revoked),
            SessionRevocation::Revoked
        );
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::AlreadyRevoked),
            SessionRevocation::AlreadyRevoked
        );
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::NotFoundOrForeign),
            SessionRevocation::NotFoundOrForeign
        );
    }
}

mod spreadsheet {
    use super::*;
    use ryframe::boot::spreadsheet::*;

    use ryframe_kernel::AppError;

    #[test]
    fn creates_streaming_artifact_without_buffering_rows() {
        let mut writer = writer_factory()
            .create("测试", &[("id", "编号")])
            .expect("应创建表格写入器");
        let mut rows = std::iter::empty::<SpreadsheetRow>();
        let progress = writer.append_rows(&mut rows).expect("空批次应可写入");
        assert_eq!(progress.total_rows, 0);

        let artifact = writer.finish().expect("应生成表格制品");
        assert!(artifact.path().is_file());
        assert!(artifact.size() > 0);
        assert_eq!(artifact.sha256().len(), 64);
        assert_eq!(artifact.data_rows(), 0);
    }

    #[tokio::test]
    async fn rejects_invalid_xlsx_source() {
        let error = document_processor()
            .validate_source(b"not-an-xlsx".to_vec(), &[("username", "用户名")])
            .await
            .expect_err("无效 XLSX 必须在上传前被拒绝");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn exports_and_reads_typed_rows_through_document_port() {
        let mut row = SpreadsheetRow::Object(Default::default());
        let SpreadsheetRow::Object(fields) = &mut row else {
            unreachable!("测试行必须是对象")
        };
        fields.insert("id".into(), SpreadsheetRow::String("42".into()));
        let processor = document_processor();
        let bytes = processor
            .export_rows(vec![row], "测试", &[("id", "编号")])
            .await
            .expect("应生成测试表格");
        let rows = processor
            .read_rows(bytes, &[("id", "编号")])
            .await
            .expect("应读回测试表格");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]
                .value
                .as_ref()
                .expect("测试行应有效")
                .get("编号")
                .and_then(SpreadsheetRow::as_str),
            Some("42")
        );
    }
}

mod tenant_config_archive {
    use super::*;
    use ryframe::boot::tenant_config_archive::*;

    #[test]
    fn controlled_archive_round_trip() {
        let codec = codec();
        let data = codec
            .build(
                "manifest.json",
                br#"{"schema":"v1"}"#,
                "resources.json",
                br#"{"items":[]}"#,
                4096,
            )
            .expect("受控归档应可生成");

        let contents = codec
            .parse(&data, "manifest.json", "resources.json", 4096, 100)
            .expect("受控归档应可解析");

        assert_eq!(contents.manifest, br#"{"schema":"v1"}"#);
        assert_eq!(contents.resources, br#"{"items":[]}"#);
    }

    #[test]
    fn archive_with_unexpected_entry_is_rejected() {
        let cursor = Cursor::new(Vec::new());
        let mut archive = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file("manifest.json", options).unwrap();
        archive.write_all(b"{}").unwrap();
        archive.start_file("unexpected.json", options).unwrap();
        archive.write_all(b"{}").unwrap();
        let data = archive.finish().unwrap().into_inner();

        let error = codec()
            .parse(&data, "manifest.json", "resources.json", 4096, 100)
            .expect_err("额外文件必须被拒绝");
        assert!(matches!(error, AppError::Validation(_)));
    }
}

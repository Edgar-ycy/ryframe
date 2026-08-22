#![cfg(feature = "destructive-reset")]

use std::{collections::BTreeMap, fs};

use async_trait::async_trait;
use chrono::Utc;
use ryframe::reset::{engine::*, ledger::*, model::*, *};
use ryframe_config::Environment;

mod command {
    use super::*;

    use ryframe::reset::model::{LegacyOwnershipPolicy, ObjectStorageResource, ResetManifest};

    #[test]
    fn command_parser_requires_exact_two_phase_shape() {
        assert_eq!(
            parse_command(vec!["plan".into()]).expect("计划命令有效"),
            Command::Plan
        );
        assert!(parse_command(vec!["execute".into()]).is_err());
        assert!(
            parse_command(vec![
                "execute".into(),
                "--plan-hash".into(),
                "0".repeat(63),
                "--confirm-reset".into(),
                "RESET".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn production_is_rejected_before_manifest_work() {
        assert!(reject_production(Environment::Prod).is_err());
        assert!(reject_production(Environment::Dev).is_ok());
        assert!(reject_production(Environment::Test).is_ok());
    }

    #[test]
    fn production_guard_precedes_configuration_and_external_runtime() {
        let source = include_str!("../src/reset/mod.rs");
        let main_body = source
            .split_once("pub async fn run")
            .and_then(|(_, suffix)| suffix.split_once("fn parse_command"))
            .map(|(body, _)| body)
            .expect("定位 reset main 函数");
        let guard = main_body
            .find("reject_production(environment)")
            .expect("生产环境保护存在");
        let config = main_body
            .find("AppConfig::load_from_env")
            .expect("配置加载存在");
        let runtime = main_body
            .find("ExternalResetRuntime::new")
            .expect("外部资源运行时存在");
        assert!(guard < config);
        assert!(guard < runtime);
    }

    #[test]
    fn redis_reset_source_excludes_broad_deletion_commands() {
        let source = include_str!("../src/reset/runtime/resources.rs");
        let forbidden = [
            ["FLUSH", "DB"].concat(),
            ["FLUSH", "ALL"].concat(),
            format!("redis::cmd({:?})", "KEYS"),
        ];
        for command in forbidden {
            assert!(!source.contains(&command), "禁止使用 Redis 广域删除命令");
        }
        assert!(source.contains("redis::cmd(\"SCAN\")"));
        assert!(source.contains("redis::cmd(\"UNLINK\")"));
        assert!(source.contains(".ryframe-reset-probe:"));
        assert!(source.contains("pub async fn prove_capabilities"));
        assert!(source.contains("raw_unlink_exact"));
    }

    #[test]
    fn execution_authorization_is_fail_closed() {
        let manifest = ResetManifest {
            manifest_version: 3,
            environment: "test".into(),
            scope_id: "test-a".into(),
            code_sha: "a".repeat(40),
            config_sha: "b".repeat(64),
            credential_version: "test-v1".into(),
            confirmation_phrase: "RESET-RYFRAME-test-test-a".into(),
            legacy_ownership: LegacyOwnershipPolicy {
                mysql_exclusive: false,
                redis_exclusive: false,
                object_storage_exclusive: false,
            },
            redis: None,
            object_storage: ObjectStorageResource {
                backend: "local".into(),
                endpoint: "unused".into(),
                use_ssl: false,
                region: String::new(),
                access_key_env: None,
                secret_key_env: None,
                prefixes: Vec::new(),
            },
            databases: Vec::new(),
        };
        let hash = "c".repeat(64);
        assert!(authorize_execute(&manifest, &hash, &hash, "wrong").is_err());
        assert!(authorize_execute(&manifest, &hash, &hash, "RESET-RYFRAME-test-test-a").is_ok());
    }
}

mod engine {
    use super::*;

    use std::{path::PathBuf, sync::Arc};

    use tokio::sync::Mutex;

    use ryframe::reset::model::{LegacyOwnershipPolicy, ObjectStorageResource};

    struct MockRuntime {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail_at: Option<ResetPhase>,
    }

    impl MockRuntime {
        async fn call(&self, phase: ResetPhase) -> ResetResult<PhaseEvidence> {
            self.calls.lock().await.push(phase.as_str());
            if self.fail_at == Some(phase) {
                return Err(ResetError::new(format!("{} 失败", phase.as_str())));
            }
            Ok(BTreeMap::from([("result".into(), "ok".into())]))
        }
    }

    #[async_trait]
    impl ResetPhases for MockRuntime {
        async fn preflight(
            &mut self,
            _manifest: &ResetManifest,
            _ledger: &ResetLedger,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::Preflight).await
        }
        async fn purge_object_storage(
            &mut self,
            _manifest: &ResetManifest,
            _progress: &mut ResourceProgress<'_>,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::ObjectStorage).await
        }
        async fn purge_redis(
            &mut self,
            _manifest: &ResetManifest,
            _progress: &mut ResourceProgress<'_>,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::Redis).await
        }
        async fn recreate_databases(
            &mut self,
            _manifest: &ResetManifest,
            _progress: &mut ResourceProgress<'_>,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::Databases).await
        }
        async fn migrate_control(
            &mut self,
            _manifest: &ResetManifest,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::ControlBaseline).await
        }
        async fn migrate_tenants(
            &mut self,
            _manifest: &ResetManifest,
        ) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::TenantBaselines).await
        }
        async fn verify(&mut self, _manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
            self.call(ResetPhase::Verification).await
        }
        async fn release(&mut self) -> ResetResult<()> {
            self.calls.lock().await.push("release");
            Ok(())
        }
    }

    fn manifest() -> ResetManifest {
        ResetManifest {
            manifest_version: 3,
            environment: "test".into(),
            scope_id: "test-a".into(),
            code_sha: "a".repeat(40),
            config_sha: "b".repeat(64),
            credential_version: "test-v1".into(),
            confirmation_phrase: "RESET-RYFRAME-test-test-a".into(),
            legacy_ownership: LegacyOwnershipPolicy {
                mysql_exclusive: false,
                redis_exclusive: false,
                object_storage_exclusive: false,
            },
            redis: None,
            object_storage: ObjectStorageResource {
                backend: "local".into(),
                endpoint: "unused".into(),
                use_ssl: false,
                region: String::new(),
                access_key_env: None,
                secret_key_env: None,
                prefixes: Vec::new(),
            },
            databases: Vec::new(),
        }
    }

    fn state_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ryframe-reset-engine-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[tokio::test]
    async fn object_failure_stops_before_redis_and_database() {
        let manifest = manifest();
        let hash = "c".repeat(64);
        let base = state_dir("object-failure");
        let store = LedgerStore::new(&base, &manifest, &hash).expect("账本路径有效");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = MockRuntime {
            calls: Arc::clone(&calls),
            fail_at: Some(ResetPhase::ObjectStorage),
        };
        assert!(
            execute(&mut runtime, &manifest, &hash, &store)
                .await
                .is_err()
        );
        assert_eq!(
            calls.lock().await.as_slice(),
            ["preflight", "object_storage", "release"]
        );
        std::fs::remove_dir_all(base).expect("清理测试状态目录");
    }

    #[tokio::test]
    async fn same_manifest_resumes_from_failed_phase() {
        let manifest = manifest();
        let hash = "d".repeat(64);
        let base = state_dir("resume");
        let store = LedgerStore::new(&base, &manifest, &hash).expect("账本路径有效");
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut first = MockRuntime {
            calls: Arc::clone(&first_calls),
            fail_at: Some(ResetPhase::Redis),
        };
        assert!(execute(&mut first, &manifest, &hash, &store).await.is_err());

        let second_calls = Arc::new(Mutex::new(Vec::new()));
        let mut second = MockRuntime {
            calls: Arc::clone(&second_calls),
            fail_at: None,
        };
        execute(&mut second, &manifest, &hash, &store)
            .await
            .expect("同一清单可以续跑");
        assert_eq!(
            second_calls.lock().await.as_slice(),
            [
                "preflight",
                "redis",
                "databases",
                "control_baseline",
                "tenant_baselines",
                "verification",
                "release"
            ]
        );
        std::fs::remove_dir_all(base).expect("清理测试状态目录");
    }
}

mod ledger {
    use super::*;

    use ryframe::reset::model::{
        DatabaseConnectionIdentity, LegacyOwnershipPolicy, ObjectStorageResource, PhysicalDatabase,
        database_resource_key,
    };

    fn manifest() -> ResetManifest {
        ResetManifest {
            manifest_version: 3,
            environment: "test".into(),
            scope_id: "test-a".into(),
            code_sha: "a".repeat(40),
            config_sha: "b".repeat(64),
            credential_version: "test-v1".into(),
            confirmation_phrase: "RESET-RYFRAME-test-test-a".into(),
            legacy_ownership: LegacyOwnershipPolicy {
                mysql_exclusive: false,
                redis_exclusive: false,
                object_storage_exclusive: false,
            },
            redis: None,
            object_storage: ObjectStorageResource {
                backend: "local".into(),
                endpoint: "unused".into(),
                use_ssl: false,
                region: String::new(),
                access_key_env: None,
                secret_key_env: None,
                prefixes: Vec::new(),
            },
            databases: Vec::new(),
        }
    }

    #[test]
    fn same_hash_resumes_and_other_identity_is_rejected() {
        let manifest = manifest();
        let base = std::env::temp_dir().join(format!(
            "ryframe-reset-ledger-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let hash = "c".repeat(64);
        let store = LedgerStore::new(&base, &manifest, &hash).expect("创建账本路径");
        let mut ledger = store.load_or_create(&manifest, &hash).expect("创建账本");
        ledger.mark_running(ResetPhase::ObjectStorage);
        ledger.mark_complete(ResetPhase::ObjectStorage, BTreeMap::new());
        store.save(&ledger).expect("保存账本");
        let resumed = store
            .load_or_create(&manifest, &hash)
            .expect("相同清单续跑");
        assert!(resumed.phase_complete(ResetPhase::ObjectStorage));

        let mut changed = manifest;
        changed.config_sha = "d".repeat(64);
        assert!(store.load_or_create(&changed, &hash).is_err());
        fs::remove_dir_all(base).expect("清理测试状态目录");
    }

    #[test]
    fn recovers_ledger_from_previous_copy_after_interrupted_replace() {
        let manifest = manifest();
        let base = std::env::temp_dir().join(format!(
            "ryframe-reset-ledger-recovery-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let hash = "e".repeat(64);
        let store = LedgerStore::new(&base, &manifest, &hash).expect("创建账本路径");
        let mut ledger = store.load_or_create(&manifest, &hash).expect("创建账本");
        ledger.mark_running(ResetPhase::ObjectStorage);
        store.save(&ledger).expect("生成上一版账本");

        let backup = backup_path(store.ledger_path()).expect("备用路径");
        if backup.exists() {
            fs::remove_file(&backup).expect("清理更早的备用账本");
        }
        fs::rename(store.ledger_path(), &backup).expect("模拟主账本替换中断");
        let recovered = store
            .load_or_create(&manifest, &hash)
            .expect("从上一版账本恢复");

        assert_eq!(
            recovered.phase(ResetPhase::ObjectStorage).status,
            PhaseStatus::Running
        );
        assert!(store.ledger_path().exists());
        assert!(backup.exists());
        fs::remove_dir_all(base).expect("清理测试状态目录");
    }

    #[test]
    fn physical_identity_mismatch_is_rejected_before_resume() {
        let manifest = manifest();
        let mut ledger = ResetLedger::new(&manifest, &"f".repeat(64));
        ledger
            .mark_resource_running("database:test", Some("identity-a"))
            .expect("首次物理身份可以持久化");
        assert!(
            ledger
                .mark_resource_running("database:test", Some("identity-b"))
                .is_err()
        );
        assert_eq!(
            ledger.resource_identity("database:test"),
            Some("identity-a")
        );
    }

    #[test]
    fn interrupted_baseline_rewinds_database_phases_and_keeps_identity() {
        let mut manifest = manifest();
        manifest.databases.push(PhysicalDatabase {
            host: "localhost".into(),
            port: 3306,
            database: "ryframe_test".into(),
            connection: DatabaseConnectionIdentity {
                username: "reset".into(),
                password_env: "RYFRAME_TEST_PASSWORD".into(),
                tls_mode: "required".into(),
                tls_ca_sha256: None,
                tls_client_cert_sha256: None,
                tls_client_key_ref_sha256: None,
            },
            target_keys: vec!["shared-control".into()],
            control_baseline: true,
            tenant_baseline: true,
            ownership_markers: BTreeMap::new(),
        });
        let mut ledger = ResetLedger::new(&manifest, &"a".repeat(64));
        let resource_key = database_resource_key(&manifest.databases[0]);
        ledger
            .mark_resource_running(&resource_key, Some("physical-a"))
            .expect("记录数据库物理身份");
        ledger.mark_resource_complete(&resource_key);
        ledger.mark_running(ResetPhase::Databases);
        ledger.mark_complete(ResetPhase::Databases, BTreeMap::new());
        ledger.mark_running(ResetPhase::ControlBaseline);
        ledger.mark_failed(
            ResetPhase::ControlBaseline,
            &ResetError::new("模拟 baseline 中断"),
        );

        assert!(
            ledger
                .rewind_interrupted_baselines(&manifest)
                .expect("中断 baseline 可以安全回退")
        );
        assert_eq!(
            ledger.phase(ResetPhase::Databases).status,
            PhaseStatus::Pending
        );
        assert_eq!(
            ledger.phase(ResetPhase::ControlBaseline).status,
            PhaseStatus::Pending
        );
        assert!(!ledger.resource_complete(&resource_key));
        assert_eq!(ledger.resource_identity(&resource_key), Some("physical-a"));
    }
}

mod model {
    use super::*;

    fn connection_identity(password_env: &str) -> DatabaseConnectionIdentity {
        DatabaseConnectionIdentity {
            username: "reset".into(),
            password_env: password_env.into(),
            tls_mode: "verify_identity".into(),
            tls_ca_sha256: Some("a".repeat(64)),
            tls_client_cert_sha256: None,
            tls_client_key_ref_sha256: None,
        }
    }

    #[test]
    fn physical_databases_are_deduplicated_and_roles_are_merged() {
        let mut databases = BTreeMap::new();
        insert_database(
            &mut databases,
            "LOCALHOST",
            3306,
            "tenant_a",
            connection_identity("TENANT_A_PASSWORD"),
            "shared-control",
            true,
            true,
            "test",
        )
        .expect("数据库有效");
        insert_database(
            &mut databases,
            "localhost",
            3306,
            "tenant_a",
            connection_identity("TENANT_A_PASSWORD"),
            "tenant-a",
            false,
            true,
            "test",
        )
        .expect("重复物理库合并");
        let database = databases.into_values().next().expect("数据库存在");
        assert_eq!(database.target_keys, ["shared-control", "tenant-a"]);
        assert!(database.control_baseline);
        assert!(database.tenant_baseline);
        assert_eq!(database.ownership_markers.len(), 2);
    }

    #[test]
    fn connection_identity_is_part_of_the_plan_hash_and_deduplication_key() {
        let baseline = connection_identity("TENANT_A_PASSWORD");
        let changed = connection_identity("TENANT_B_PASSWORD");
        assert_ne!(
            sha256_hex(&canonical_json(&baseline).expect("编码连接身份")),
            sha256_hex(&canonical_json(&changed).expect("编码连接身份"))
        );

        let mut databases = BTreeMap::new();
        insert_database(
            &mut databases,
            "localhost",
            3306,
            "tenant_a",
            baseline,
            "shared-control",
            true,
            true,
            "test",
        )
        .expect("首次登记数据库");
        assert!(
            insert_database(
                &mut databases,
                "localhost",
                3306,
                "tenant_a",
                changed,
                "tenant-a",
                false,
                true,
                "test",
            )
            .is_err()
        );
    }

    #[test]
    fn system_schemas_and_unsafe_identifiers_are_rejected() {
        assert!(validate_database_identity("localhost", 3306, "mysql").is_err());
        assert!(validate_database_identity("localhost", 3306, "tenant-a").is_err());
        assert!(validate_database_identity("", 3306, "tenant_a").is_err());
    }

    #[test]
    fn canonical_hash_is_deterministic() {
        let bytes = canonical_json(&vec!["a", "b"]).expect("编码清单");
        assert_eq!(sha256_hex(&bytes), sha256_hex(&bytes));
    }
}

mod mysql {
    use ryframe::reset::runtime::mysql::*;

    use ryframe_config::DbConnection;

    #[test]
    fn ddl_identifier_quoting_is_strict() {
        assert_eq!(
            quote_identifier("tenant_a").expect("标识符有效"),
            "`tenant_a`"
        );
        assert!(quote_identifier("tenant-a").is_err());
        assert!(quote_identifier("tenant`; DROP DATABASE x").is_err());
        assert!(quote_identifier("mysql").is_ok());
    }

    #[test]
    fn deduplication_requires_identical_credentials() {
        let left = DbConnection {
            username: "reset".into(),
            password: "secret-a".into(),
            ..DbConnection::default()
        };
        let mut right = left.clone();
        assert!(same_credentials(&left, &right));
        right.password = "secret-b".into();
        assert!(!same_credentials(&left, &right));
    }

    #[test]
    fn lower_case_table_names_accepts_only_mysql_modes() {
        assert_eq!(parse_lower_case_table_names("0").expect("模式有效"), 0);
        assert_eq!(parse_lower_case_table_names("1").expect("模式有效"), 1);
        assert_eq!(parse_lower_case_table_names("2").expect("模式有效"), 2);
        for invalid in ["-1", "3", " 1", "true"] {
            assert!(parse_lower_case_table_names(invalid).is_err());
        }
    }
}

mod resources {
    use ryframe::reset::runtime::resources::*;

    #[test]
    fn unlink_candidates_cannot_escape_namespace() {
        let namespace = "ryframe:{test-a}:";
        assert!(
            validate_physical_keys(
                &[
                    "ryframe:{test-a}:jobs:1".into(),
                    "ryframe:{test-a}:cache".into()
                ],
                namespace,
            )
            .is_ok()
        );
        assert!(validate_physical_keys(&["ryframe:{test-b}:sentinel".into()], namespace).is_err());
    }

    #[test]
    fn capability_probe_is_plan_bound_and_cannot_escape_namespace() {
        let namespace = "ryframe:{test-a}:";
        let key = reset_probe_key(namespace, &"a".repeat(64)).expect("探针键有效");
        assert!(key.starts_with(namespace));
        assert!(key.ends_with(&"a".repeat(64)));
        assert!(reset_probe_key(namespace, "not-a-hash").is_err());
    }

    #[test]
    fn ownership_markers_are_never_selected_for_deletion() {
        let marker = "ryframe:{test-a}:.ryframe-owner";
        assert!(!is_deletable_object_key(marker, marker));
        assert!(is_deletable_object_key(
            "ryframe:{test-a}:exports/1",
            marker
        ));

        let mut keys = vec![marker.into(), "ryframe:{test-a}:jobs:1".into()];
        retain_deletable_redis_keys(&mut keys, marker);
        assert_eq!(keys, ["ryframe:{test-a}:jobs:1"]);
    }
}

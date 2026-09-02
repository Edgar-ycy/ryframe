use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use ::redis::aio::ConnectionManager;
use ryframe_adapters::{
    RedisClient,
    storage::{
        LocalObjectStorage, MAX_OBJECT_LIST_PAGE_SIZE, ObjectStorage, S3Config, S3ObjectStorage,
    },
};
use ryframe_config::{AppConfig, RedisConfig, StorageBackend};

use crate::reset::{
    ResetError, ResetResult,
    engine::{PhaseEvidence, ResourceProgress},
    ledger::ResetLedger,
    model::{
        OBJECT_STORAGE_ACCESS_KEY_ENV, OBJECT_STORAGE_SECRET_KEY_ENV, ObjectPrefix,
        REDIS_PASSWORD_ENV, ResetManifest, normalize_host, object_resource_key,
        public_material_sha256, redis_resource_key, secret_reference_sha256,
    },
};

use super::mysql::MysqlReset;

const MAX_PREFIX_DELETE_BATCHES: usize = 100_000;
const MAX_REDIS_SCAN_PAGES: usize = 100_000;
const REDIS_SCAN_BATCH_SIZE: usize = 512;
const MAX_MARKER_BYTES: usize = 1_024;
const MAX_SENTINEL_BYTES: usize = 4_096;

fn validate_object_storage_identity(
    config: &AppConfig,
    manifest: &ResetManifest,
) -> ResetResult<()> {
    let resource = &manifest.object_storage;
    if resource.backend != config.object_storage.backend.as_str()
        || resource.use_ssl != config.object_storage.use_ssl
        || resource.region != config.object_storage.region.trim()
    {
        return Err(ResetError::new(
            "对象存储运行时参数与不可变清单不一致，请重新运行 plan",
        ));
    }
    if config.object_storage.backend == StorageBackend::Local {
        if resource.access_key_env.is_some() || resource.secret_key_env.is_some() {
            return Err(ResetError::new("本地对象存储清单不能引用远端凭据"));
        }
        return Ok(());
    }
    if resource.endpoint != config.object_storage.endpoint.trim()
        || resource.access_key_env.as_deref() != Some(OBJECT_STORAGE_ACCESS_KEY_ENV)
        || resource.secret_key_env.as_deref() != Some(OBJECT_STORAGE_SECRET_KEY_ENV)
    {
        return Err(ResetError::new("对象存储端点或凭据引用与不可变清单不一致"));
    }
    require_secret_matches(
        OBJECT_STORAGE_ACCESS_KEY_ENV,
        &config.object_storage.access_key,
        "对象存储 access key",
    )?;
    require_secret_matches(
        OBJECT_STORAGE_SECRET_KEY_ENV,
        &config.object_storage.secret_key,
        "对象存储 secret key",
    )
}

fn validate_redis_identity(config: &RedisConfig, manifest: &ResetManifest) -> ResetResult<()> {
    let resource = manifest
        .redis
        .as_ref()
        .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
    if resource.host != normalize_host(&config.host)
        || resource.port != config.port
        || resource.database != config.database
        || resource.namespace != config.namespace()
        || resource.tls != config.tls
        || resource.tls_ca_sha256 != public_material_sha256(config.tls_ca.as_deref())?
        || resource.tls_client_cert_sha256
            != public_material_sha256(config.tls_client_cert.as_deref())?
        || resource.tls_client_key_ref_sha256
            != secret_reference_sha256(config.tls_client_key.as_deref())?
    {
        return Err(ResetError::new(
            "Redis 非秘密连接参数与不可变清单不一致，请重新运行 plan",
        ));
    }
    match resource.password_env.as_deref() {
        Some(REDIS_PASSWORD_ENV) => {
            require_secret_matches(REDIS_PASSWORD_ENV, &config.password, "Redis 密码")
        }
        None if config.password.is_empty() => Ok(()),
        _ => Err(ResetError::new("Redis 密码引用与不可变清单不一致")),
    }
}

fn require_secret_matches(name: &str, configured: &str, label: &str) -> ResetResult<()> {
    let value = std::env::var(name)
        .map_err(|_| ResetError::new(format!("{label}环境变量缺失或编码无效")))?;
    if value.is_empty() || value != configured {
        return Err(ResetError::new(format!(
            "{label}必须来自不可变清单声明的环境变量"
        )));
    }
    Ok(())
}

pub struct StorageReset {
    storage: Arc<dyn ObjectStorage>,
}

impl StorageReset {
    pub fn new(config: &AppConfig, manifest: &ResetManifest) -> ResetResult<Self> {
        validate_object_storage_identity(config, manifest)?;
        let storage: Arc<dyn ObjectStorage> = match config.object_storage.backend {
            StorageBackend::Local => Arc::new(LocalObjectStorage::new(PathBuf::from(
                &manifest.object_storage.endpoint,
            ))),
            StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => Arc::new(
                S3ObjectStorage::new(S3Config {
                    endpoint: config.object_storage.endpoint.trim().to_owned(),
                    access_key: config.object_storage.access_key.clone(),
                    secret_key: config.object_storage.secret_key.clone(),
                    use_ssl: config.object_storage.use_ssl,
                    root_ca_pem: None,
                    region: config.object_storage.region.trim().to_owned(),
                    request_timeout_secs: config.object_storage.request_timeout_secs,
                })
                .map_err(|_| ResetError::new("对象存储配置无法创建安全客户端"))?,
            ),
        };
        Ok(Self { storage })
    }

    pub async fn inspect(&self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        for item in &manifest.object_storage.prefixes {
            self.storage
                .readiness_check(&item.bucket)
                .await
                .map_err(|_| ResetError::new("对象存储桶不存在或不可读"))?;
            let marker_exists = self
                .storage
                .exists(&item.bucket, &item.ownership_marker_key)
                .await
                .map_err(|_| ResetError::new("对象存储所有权预检失败"))?;
            if marker_exists {
                let marker = self
                    .storage
                    .get_bounded(&item.bucket, &item.ownership_marker_key, MAX_MARKER_BYTES)
                    .await
                    .map_err(|_| ResetError::new("无法读取对象存储所有权 marker"))?;
                if marker.as_slice() != item.ownership_marker.as_bytes() {
                    return Err(ResetError::new(format!(
                        "对象存储桶 {} 的所有权 marker 不匹配",
                        item.bucket
                    )));
                }
            } else if manifest.legacy_ownership.object_storage_exclusive {
            } else {
                return Err(ResetError::new(format!(
                    "对象存储桶 {} 缺少 scope marker；仅能通过明确的 dev/test 旧资源独占配置接管",
                    item.bucket
                )));
            }
        }
        Ok(PhaseEvidence::from([
            (
                "bucket_count".into(),
                manifest.object_storage.prefixes.len().to_string(),
            ),
            ("ownership".into(), "verified".into()),
        ]))
    }

    /// 在其他外部资源均完成只读预检后，证明五个桶具备 scoped 写入、列举和删除能力。
    pub async fn prove_capabilities(
        &self,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        for item in &manifest.object_storage.prefixes {
            guard.assert_locks_held().await?;
            if !self
                .storage
                .exists(&item.bucket, &item.ownership_marker_key)
                .await
                .map_err(|_| ResetError::new("无法检查对象存储所有权 marker"))?
            {
                if !manifest.legacy_ownership.object_storage_exclusive {
                    return Err(ResetError::new("对象存储缺少可验证的所有权 marker"));
                }
                self.put_marker(item).await?;
            }
            self.verify_marker_before_purge(item).await?;
            let probe_prefix = format!("{}.ryframe-reset-probe/{}/", item.prefix, ledger.plan_hash);
            let probe = format!("{probe_prefix}capability");
            let probe_value = format!("ryframe-reset-probe:v1:{}", ledger.plan_hash);
            if self
                .storage
                .exists(&item.bucket, &probe)
                .await
                .map_err(|_| ResetError::new("无法检查对象存储 scoped 能力探针"))?
            {
                let existing = self
                    .storage
                    .get_bounded(&item.bucket, &probe, MAX_MARKER_BYTES)
                    .await
                    .map_err(|_| ResetError::new("无法读取对象存储 scoped 能力探针"))?;
                if existing.as_slice() != probe_value.as_bytes() {
                    return Err(ResetError::new(
                        "对象存储 scoped 能力探针路径已被非 reset 数据占用",
                    ));
                }
            }
            guard.assert_locks_held().await?;
            if self
                .storage
                .put_control(&item.bucket, &probe, probe_value.as_bytes(), "text/plain")
                .await
                .is_err()
            {
                self.cleanup_probe(item, &probe, guard).await?;
                return Err(ResetError::new("对象存储 scoped 写入能力预检失败"));
            }
            let page = match self
                .storage
                .list_page(&item.bucket, &probe_prefix, None, 1)
                .await
            {
                Ok(page) => page,
                Err(_) => {
                    self.cleanup_probe(item, &probe, guard).await?;
                    return Err(ResetError::new("对象存储有界列举能力预检失败"));
                }
            };
            if !page.keys.iter().any(|key| key == &probe) {
                self.cleanup_probe(item, &probe, guard).await?;
                return Err(ResetError::new("对象存储列举未返回刚写入的 scoped 探针"));
            }
            guard.assert_locks_held().await?;
            if self.storage.delete(&item.bucket, &probe).await.is_err() {
                self.cleanup_probe(item, &probe, guard).await?;
                return Err(ResetError::new("对象存储 scoped 删除能力预检失败"));
            }
            let remaining = match self.storage.exists(&item.bucket, &probe).await {
                Ok(remaining) => remaining,
                Err(_) => {
                    self.cleanup_probe(item, &probe, guard).await?;
                    return Err(ResetError::new("对象存储删除结果验证失败"));
                }
            };
            if remaining {
                self.cleanup_probe(item, &probe, guard).await?;
                return Err(ResetError::new("对象存储 scoped 删除探针后仍然存在"));
            }
        }
        Ok(())
    }

    pub async fn purge(
        &self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
        guard: &MysqlReset,
    ) -> ResetResult<PhaseEvidence> {
        let mut deleted = 0_usize;
        for item in &manifest.object_storage.prefixes {
            let resource_key = object_resource_key(item);
            if progress.is_complete(&resource_key) {
                continue;
            }
            guard.assert_locks_held().await?;
            self.verify_marker_before_purge(item).await?;
            progress.begin(&resource_key)?;
            let mut batches = 0_usize;
            loop {
                guard.assert_locks_held().await?;
                self.verify_marker_before_purge(item).await?;
                if batches >= MAX_PREFIX_DELETE_BATCHES {
                    return Err(ResetError::new("对象存储精确前缀清理超过安全批次上限"));
                }
                let page = self
                    .storage
                    .list_page(&item.bucket, &item.prefix, None, MAX_OBJECT_LIST_PAGE_SIZE)
                    .await
                    .map_err(|_| ResetError::new("对象存储精确前缀列举失败"))?;
                let mut deleted_in_batch = 0_usize;
                for key in page.keys {
                    if !is_deletable_object_key(&key, &item.ownership_marker_key) {
                        continue;
                    }
                    self.storage
                        .delete(&item.bucket, &key)
                        .await
                        .map_err(|_| ResetError::new("对象存储精确对象清理失败"))?;
                    deleted_in_batch = deleted_in_batch.saturating_add(1);
                }
                deleted = deleted.saturating_add(deleted_in_batch);
                batches += 1;
                if deleted_in_batch == 0 {
                    if page.next_cursor.is_some() {
                        return Err(ResetError::new("对象存储分页未取得可清理对象"));
                    }
                    break;
                }
            }
            self.verify_only_marker(item).await?;
            progress.complete(&resource_key)?;
        }
        Ok(PhaseEvidence::from([
            ("deleted_objects".into(), deleted.to_string()),
            (
                "verified_empty_prefixes".into(),
                manifest.object_storage.prefixes.len().to_string(),
            ),
        ]))
    }

    pub async fn verify(&self, manifest: &ResetManifest) -> ResetResult<()> {
        for item in &manifest.object_storage.prefixes {
            self.verify_only_marker(item).await?;
        }
        Ok(())
    }

    async fn put_marker(&self, item: &ObjectPrefix) -> ResetResult<()> {
        self.storage
            .put_control(
                &item.bucket,
                &item.ownership_marker_key,
                item.ownership_marker.as_bytes(),
                "text/plain",
            )
            .await
            .map_err(|_| ResetError::new("无法写入对象存储所有权 marker"))
    }

    async fn verify_marker_before_purge(&self, item: &ObjectPrefix) -> ResetResult<()> {
        let exists = self
            .storage
            .exists(&item.bucket, &item.ownership_marker_key)
            .await
            .map_err(|_| ResetError::new("对象存储所有权 marker 破坏前复验失败"))?;
        if !exists {
            return Err(ResetError::new(format!(
                "对象存储桶 {} 在清理前缺少 scope marker",
                item.bucket
            )));
        }
        let marker = self
            .storage
            .get_bounded(&item.bucket, &item.ownership_marker_key, MAX_MARKER_BYTES)
            .await
            .map_err(|_| ResetError::new("无法在清理前读取对象存储所有权 marker"))?;
        if marker.as_slice() != item.ownership_marker.as_bytes() {
            return Err(ResetError::new(format!(
                "对象存储桶 {} 在清理前的所有权 marker 不匹配",
                item.bucket
            )));
        }
        Ok(())
    }

    async fn cleanup_probe(
        &self,
        item: &ObjectPrefix,
        probe: &str,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        guard.assert_locks_held().await?;
        self.storage
            .delete(&item.bucket, probe)
            .await
            .map_err(|_| ResetError::new("对象存储 scoped 能力探针清理失败"))?;
        if self
            .storage
            .exists(&item.bucket, probe)
            .await
            .map_err(|_| ResetError::new("对象存储 scoped 能力探针清理验证失败"))?
        {
            return Err(ResetError::new("对象存储 scoped 能力探针清理后仍然存在"));
        }
        Ok(())
    }

    async fn verify_only_marker(&self, item: &ObjectPrefix) -> ResetResult<()> {
        let page = self
            .storage
            .list_page(&item.bucket, &item.prefix, None, 2)
            .await
            .map_err(|_| ResetError::new("无法验证对象存储精确前缀"))?;
        if page.next_cursor.is_some()
            || page.keys.as_slice() != [item.ownership_marker_key.as_str()]
        {
            return Err(ResetError::new(format!(
                "对象存储桶 {} 的 scope 前缀并非空资源状态",
                item.bucket
            )));
        }
        let marker = self
            .storage
            .get_bounded(&item.bucket, &item.ownership_marker_key, MAX_MARKER_BYTES)
            .await
            .map_err(|_| ResetError::new("无法复验对象存储所有权 marker"))?;
        if marker.as_slice() != item.ownership_marker.as_bytes() {
            return Err(ResetError::new("对象存储所有权 marker 复验失败"));
        }
        Ok(())
    }
}

mod redis;

pub use redis::{
    RedisReset, is_deletable_object_key, reset_probe_key, retain_deletable_redis_keys,
    validate_physical_keys,
};

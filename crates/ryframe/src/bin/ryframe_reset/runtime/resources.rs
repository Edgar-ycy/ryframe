use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use redis::aio::ConnectionManager;
use ryframe_adapters::{
    RedisClient,
    storage::{
        LocalObjectStorage, MAX_OBJECT_LIST_PAGE_SIZE, ObjectStorage, S3Config, S3ObjectStorage,
    },
};
use ryframe_config::{AppConfig, RedisConfig, StorageBackend};

use crate::{
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
                    region: config.object_storage.region.trim().to_owned(),
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

pub struct RedisReset {
    client: RedisClient,
    sentinel_key: String,
    sentinel_value: Vec<u8>,
}

impl RedisReset {
    pub async fn connect_and_inspect(
        config: &RedisConfig,
        sentinel_key: &str,
        manifest: &ResetManifest,
        legacy_exclusive: bool,
    ) -> ResetResult<(Self, PhaseEvidence)> {
        validate_redis_identity(config, manifest)?;
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        if crate::model::sha256_hex(sentinel_key.as_bytes()) != resource.outside_sentinel_key_sha256
            || sentinel_key.starts_with(&resource.namespace)
        {
            return Err(ResetError::new("Redis scope 外哨兵键与不可变清单不匹配"));
        }
        let client = RedisClient::connect(config)
            .await
            .map_err(|_| ResetError::new("Redis 连接预检失败"))?;
        client
            .ping()
            .await
            .map_err(|_| ResetError::new("Redis PING 预检失败"))?;
        let marker = raw_get_bounded(
            client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?;
        match marker {
            Some(value) if value.as_slice() == resource.ownership_marker.as_bytes() => {}
            Some(_) => return Err(ResetError::new("Redis scope 所有权 marker 不匹配")),
            None if legacy_exclusive => {}
            None => {
                return Err(ResetError::new(
                    "Redis namespace 缺少所有权 marker；仅能通过明确的 dev/test 旧资源独占配置接管",
                ));
            }
        }
        let sentinel_value =
            raw_get_bounded(client.conn().clone(), sentinel_key, MAX_SENTINEL_BYTES)
                .await?
                .ok_or_else(|| ResetError::new("Redis scope 外哨兵键不存在"))?;
        Ok((
            Self {
                client,
                sentinel_key: sentinel_key.to_owned(),
                sentinel_value,
            },
            PhaseEvidence::from([
                ("namespace".into(), resource.namespace.clone()),
                ("outside_sentinel".into(), "verified".into()),
            ]),
        ))
    }

    pub async fn prove_capabilities(
        &self,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify_ownership_before_purge(resource, manifest.legacy_ownership.redis_exclusive)
            .await?;
        if raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?
        .is_none()
        {
            guard.assert_locks_held().await?;
            self.verify_sentinel_unchanged().await?;
            raw_set(
                self.client.conn().clone(),
                &resource.ownership_marker_key,
                resource.ownership_marker.as_bytes(),
            )
            .await?;
        }
        self.verify_ownership_before_purge(resource, false).await?;
        let probe = reset_probe_key(&resource.namespace, &ledger.plan_hash)?;
        let probe_value = format!("ryframe-reset-probe:v1:{}", ledger.plan_hash);
        match raw_get_bounded(self.client.conn().clone(), &probe, MAX_MARKER_BYTES).await? {
            None => {}
            Some(value) if value.as_slice() == probe_value.as_bytes() => {}
            Some(_) => {
                return Err(ResetError::new(
                    "Redis scoped 能力探针键已存在，拒绝覆盖非本次 reset 数据",
                ));
            }
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        if let Err(error) =
            raw_set(self.client.conn().clone(), &probe, probe_value.as_bytes()).await
        {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(error);
        }
        let keys = match scan_scope_keys(self.client.conn().clone(), &probe, &resource.namespace, 1)
            .await
        {
            Ok(keys) => keys,
            Err(error) => {
                self.cleanup_probe(&probe, &resource.namespace, guard)
                    .await?;
                return Err(error);
            }
        };
        if keys.as_slice() != [probe.as_str()] {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis SCAN 未精确返回 scoped 能力探针"));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        let removed =
            match raw_unlink_exact(self.client.conn().clone(), &probe, &resource.namespace).await {
                Ok(removed) => removed,
                Err(error) => {
                    self.cleanup_probe(&probe, &resource.namespace, guard)
                        .await?;
                    return Err(error);
                }
            };
        if removed != 1 {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis scoped 能力探针删除数量不匹配"));
        }
        let remaining =
            match raw_get_bounded(self.client.conn().clone(), &probe, MAX_MARKER_BYTES).await {
                Ok(remaining) => remaining,
                Err(error) => {
                    self.cleanup_probe(&probe, &resource.namespace, guard)
                        .await?;
                    return Err(error);
                }
            };
        if remaining.is_some() {
            self.cleanup_probe(&probe, &resource.namespace, guard)
                .await?;
            return Err(ResetError::new("Redis scoped 能力探针删除后仍然存在"));
        }
        self.verify_sentinel_unchanged().await
    }

    pub async fn purge(
        &self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
        guard: &MysqlReset,
    ) -> ResetResult<PhaseEvidence> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        let resource_key = redis_resource_key(resource);
        if progress.is_complete(&resource_key) {
            self.verify(manifest).await?;
            return Ok(PhaseEvidence::from([("deleted_keys".into(), "0".into())]));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify_ownership_before_purge(resource, false).await?;
        progress.begin(&resource_key)?;
        let pattern = format!("{}*", resource.namespace);
        let mut deleted = 0_u64;
        for _ in 0..4 {
            deleted = deleted
                .saturating_add(scan_and_unlink_scope(self, guard, resource, &pattern).await?);
            if scan_scope_keys(self.client.conn().clone(), &pattern, &resource.namespace, 2)
                .await?
                .as_slice()
                == [resource.ownership_marker_key.as_str()]
            {
                break;
            }
        }
        if scan_scope_keys(self.client.conn().clone(), &pattern, &resource.namespace, 2)
            .await?
            .as_slice()
            != [resource.ownership_marker_key.as_str()]
        {
            return Err(ResetError::new("Redis scope 清理后仍有键"));
        }
        guard.assert_locks_held().await?;
        self.verify_sentinel_unchanged().await?;
        self.verify(manifest).await?;
        progress.complete(&resource_key)?;
        Ok(PhaseEvidence::from([
            ("deleted_keys".into(), deleted.to_string()),
            ("outside_sentinel".into(), "unchanged".into()),
        ]))
    }

    pub async fn verify(&self, manifest: &ResetManifest) -> ResetResult<()> {
        let resource = manifest
            .redis
            .as_ref()
            .ok_or_else(|| ResetError::new("Redis manifest 缺失"))?;
        let keys = scan_scope_keys(
            self.client.conn().clone(),
            &format!("{}*", resource.namespace),
            &resource.namespace,
            2,
        )
        .await?;
        if keys.as_slice() != [resource.ownership_marker_key.as_str()] {
            return Err(ResetError::new("Redis scope 并非仅保留所有权 marker"));
        }
        let marker = raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?
        .ok_or_else(|| ResetError::new("Redis 所有权 marker 不存在"))?;
        if marker.as_slice() != resource.ownership_marker.as_bytes() {
            return Err(ResetError::new("Redis 所有权 marker 复验失败"));
        }
        self.verify_sentinel_unchanged().await
    }

    async fn verify_sentinel_unchanged(&self) -> ResetResult<()> {
        let sentinel = raw_get_bounded(
            self.client.conn().clone(),
            &self.sentinel_key,
            MAX_SENTINEL_BYTES,
        )
        .await?
        .ok_or_else(|| ResetError::new("Redis scope 外哨兵键在重建后消失"))?;
        if sentinel != self.sentinel_value {
            return Err(ResetError::new("Redis scope 外哨兵值在重建期间发生变化"));
        }
        Ok(())
    }

    async fn verify_ownership_before_purge(
        &self,
        resource: &crate::model::RedisResource,
        legacy_exclusive: bool,
    ) -> ResetResult<()> {
        let marker = raw_get_bounded(
            self.client.conn().clone(),
            &resource.ownership_marker_key,
            MAX_MARKER_BYTES,
        )
        .await?;
        match marker {
            Some(value) if value.as_slice() == resource.ownership_marker.as_bytes() => Ok(()),
            Some(_) => Err(ResetError::new(
                "Redis scope 在清理前的所有权 marker 不匹配",
            )),
            None if legacy_exclusive => Ok(()),
            None => Err(ResetError::new("Redis scope 在清理前缺少所有权 marker")),
        }
    }

    async fn cleanup_probe(
        &self,
        probe: &str,
        namespace: &str,
        guard: &MysqlReset,
    ) -> ResetResult<()> {
        guard.assert_locks_held().await?;
        let cleanup = raw_unlink_exact(self.client.conn().clone(), probe, namespace).await;
        let sentinel = self.verify_sentinel_unchanged().await;
        sentinel?;
        cleanup.map(|_| ())
    }
}

fn reset_probe_key(namespace: &str, plan_hash: &str) -> ResetResult<String> {
    if namespace.is_empty()
        || plan_hash.len() != 64
        || !plan_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ResetError::new(
            "Redis 能力探针收到非法 namespace 或 plan hash",
        ));
    }
    Ok(format!("{namespace}.ryframe-reset-probe:{plan_hash}"))
}

async fn scan_and_unlink_scope(
    redis: &RedisReset,
    guard: &MysqlReset,
    resource: &crate::model::RedisResource,
    pattern: &str,
) -> ResetResult<u64> {
    let mut connection = redis.client.conn().clone();
    let mut cursor = 0_u64;
    let mut pages = 0_usize;
    let mut deleted = 0_u64;
    loop {
        guard.assert_locks_held().await?;
        redis.verify_sentinel_unchanged().await?;
        if pages >= MAX_REDIS_SCAN_PAGES {
            return Err(ResetError::new("Redis SCAN 超过安全页数上限"));
        }
        let (next, mut keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(REDIS_SCAN_BATCH_SIZE)
            .query_async(&mut connection)
            .await
            .map_err(|_| ResetError::new("Redis scope SCAN 失败"))?;
        validate_physical_keys(&keys, &resource.namespace)?;
        retain_deletable_redis_keys(&mut keys, &resource.ownership_marker_key);
        if !keys.is_empty() {
            guard.assert_locks_held().await?;
            redis.verify_sentinel_unchanged().await?;
            redis.verify_ownership_before_purge(resource, false).await?;
            let mut command = redis::cmd("UNLINK");
            for key in &keys {
                command.arg(key);
            }
            let count: u64 = command
                .query_async(&mut connection)
                .await
                .map_err(|_| ResetError::new("Redis scope UNLINK 失败"))?;
            deleted = deleted.saturating_add(count);
        }
        pages += 1;
        if next == 0 {
            return Ok(deleted);
        }
        cursor = next;
    }
}

async fn scan_scope_keys(
    mut connection: ConnectionManager,
    pattern: &str,
    namespace: &str,
    maximum: usize,
) -> ResetResult<Vec<String>> {
    let mut cursor = 0_u64;
    let mut pages = 0_usize;
    let mut keys = BTreeSet::new();
    loop {
        if pages >= MAX_REDIS_SCAN_PAGES {
            return Err(ResetError::new("Redis 验证 SCAN 超过安全页数上限"));
        }
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(REDIS_SCAN_BATCH_SIZE)
            .query_async(&mut connection)
            .await
            .map_err(|_| ResetError::new("Redis scope 验证 SCAN 失败"))?;
        validate_physical_keys(&batch, namespace)?;
        keys.extend(batch);
        if keys.len() > maximum {
            break;
        }
        pages += 1;
        if next == 0 {
            break;
        }
        cursor = next;
    }
    Ok(keys.into_iter().collect())
}

fn validate_physical_keys(keys: &[String], namespace: &str) -> ResetResult<()> {
    if keys.iter().any(|key| !key.starts_with(namespace)) {
        return Err(ResetError::new(
            "Redis SCAN 返回 scope 外键，拒绝执行 UNLINK",
        ));
    }
    Ok(())
}

fn is_deletable_object_key(key: &str, ownership_marker_key: &str) -> bool {
    key != ownership_marker_key
}

fn retain_deletable_redis_keys(keys: &mut Vec<String>, ownership_marker_key: &str) {
    keys.retain(|key| key != ownership_marker_key);
}

async fn raw_get_bounded(
    mut connection: ConnectionManager,
    key: &str,
    max_bytes: usize,
) -> ResetResult<Option<Vec<u8>>> {
    if max_bytes == 0 || max_bytes > i64::MAX as usize {
        return Err(ResetError::new("Redis 有界读取上限无效"));
    }
    let initial_length: u64 = redis::cmd("STRLEN")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键长度读取失败"))?;
    if initial_length > max_bytes as u64 {
        return Err(ResetError::new("Redis 控制键超过有界读取上限"));
    }
    let value: Vec<u8> = redis::cmd("GETRANGE")
        .arg(key)
        .arg(0)
        .arg(max_bytes as i64)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键有界读取失败"))?;
    if value.len() > max_bytes {
        return Err(ResetError::new("Redis 控制键在读取期间超过上限"));
    }
    let exists: bool = redis::cmd("EXISTS")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键存在性复验失败"))?;
    if !exists {
        return if value.is_empty() {
            Ok(None)
        } else {
            Err(ResetError::new("Redis 控制键在读取期间发生变化"))
        };
    }
    let final_length: u64 = redis::cmd("STRLEN")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确键长度复验失败"))?;
    if final_length > max_bytes as u64
        || final_length != initial_length
        || final_length != value.len() as u64
    {
        return Err(ResetError::new("Redis 控制键在有界读取期间发生变化"));
    }
    Ok(Some(value))
}

async fn raw_set(mut connection: ConnectionManager, key: &str, value: &[u8]) -> ResetResult<()> {
    redis::cmd("SET")
        .arg(key)
        .arg(value)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis scoped 键写入失败"))
}

async fn raw_unlink_exact(
    mut connection: ConnectionManager,
    key: &str,
    namespace: &str,
) -> ResetResult<u64> {
    if !key.starts_with(namespace) {
        return Err(ResetError::new("Redis 精确 UNLINK 键不属于当前 namespace"));
    }
    redis::cmd("UNLINK")
        .arg(key)
        .query_async(&mut connection)
        .await
        .map_err(|_| ResetError::new("Redis 精确 scoped 键 UNLINK 失败"))
}

#[cfg(test)]
mod tests {
    use super::{
        is_deletable_object_key, reset_probe_key, retain_deletable_redis_keys,
        validate_physical_keys,
    };

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

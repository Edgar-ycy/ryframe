mod mysql;
mod resources;

use async_trait::async_trait;
use ryframe_config::AppConfig;

use crate::{
    ResetError, ResetResult,
    engine::{PhaseEvidence, ResetPhases, ResourceProgress},
    ledger::ResetLedger,
    model::ResetManifest,
};

use self::{
    mysql::MysqlReset,
    resources::{RedisReset, StorageReset},
};

pub struct ExternalResetRuntime {
    config: AppConfig,
    storage: StorageReset,
    redis: Option<RedisReset>,
    mysql: MysqlReset,
}

impl ExternalResetRuntime {
    pub fn new(config: AppConfig, manifest: &ResetManifest) -> ResetResult<Self> {
        let storage = StorageReset::new(&config, manifest)?;
        Ok(Self {
            config,
            storage,
            redis: None,
            mysql: MysqlReset::new(),
        })
    }
}

#[async_trait]
impl ResetPhases for ExternalResetRuntime {
    async fn preflight(
        &mut self,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
    ) -> ResetResult<PhaseEvidence> {
        let mut evidence = self.storage.inspect(manifest).await?;

        match (self.config.redis.as_ref(), manifest.redis.as_ref()) {
            (Some(config), Some(_)) => {
                let sentinel = self
                    .config
                    .reset
                    .redis_outside_sentinel_key
                    .as_deref()
                    .ok_or_else(|| ResetError::new("Redis scope 外哨兵键配置缺失"))?;
                let (redis, redis_evidence) = RedisReset::connect_and_inspect(
                    config,
                    sentinel,
                    manifest,
                    manifest.legacy_ownership.redis_exclusive,
                )
                .await?;
                evidence.extend(redis_evidence);
                self.redis = Some(redis);
            }
            (None, None) => {
                evidence.insert("redis".into(), "not_configured".into());
            }
            _ => {
                return Err(ResetError::new("Redis 运行时配置与不可变 manifest 不一致"));
            }
        }

        let mysql_evidence = self.mysql.preflight(&self.config, manifest, ledger).await?;
        evidence.extend(mysql_evidence);

        // 删除权限探针必须放在所有数据库、Redis 和对象存储只读安全检查之后。
        // 它只删除刚由本 manifest 在精确 scope 内写入的探针对象。
        self.storage
            .prove_capabilities(manifest, ledger, &self.mysql)
            .await?;
        evidence.insert("object_storage_capabilities".into(), "verified".into());
        if let Some(redis) = &self.redis {
            redis
                .prove_capabilities(manifest, ledger, &self.mysql)
                .await?;
            evidence.insert("redis_scoped_capabilities".into(), "verified".into());
        }
        Ok(evidence)
    }

    async fn purge_object_storage(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence> {
        self.storage.purge(manifest, progress, &self.mysql).await
    }

    async fn purge_redis(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence> {
        match &self.redis {
            Some(redis) => redis.purge(manifest, progress, &self.mysql).await,
            None if manifest.redis.is_none() => Ok(PhaseEvidence::from([(
                "redis".into(),
                "not_configured".into(),
            )])),
            None => Err(ResetError::new("Redis 未完成预检")),
        }
    }

    async fn recreate_databases(
        &mut self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence> {
        self.mysql.recreate(manifest, progress).await
    }

    async fn migrate_control(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.mysql.migrate_control(manifest).await
    }

    async fn migrate_tenants(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.mysql.migrate_tenants(manifest).await
    }

    async fn verify(&mut self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.storage.verify(manifest).await?;
        if let Some(redis) = &self.redis {
            redis.verify(manifest).await?;
        }
        let mut evidence = self.mysql.verify(manifest).await?;
        evidence.insert("object_storage".into(), "verified".into());
        evidence.insert(
            "redis".into(),
            if self.redis.is_some() {
                "verified"
            } else {
                "not_configured"
            }
            .into(),
        );
        Ok(evidence)
    }

    async fn release(&mut self) -> ResetResult<()> {
        self.mysql.release().await
    }
}

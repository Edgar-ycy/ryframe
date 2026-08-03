use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, ConfigFilter, ConfigRepository,
    entities::config,
};
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};

use crate::{AuthorizationCache, NamespaceCacheLookup};

/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub key: String,
    pub value: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<config::Model> for ConfigVo {
    fn from(c: config::Model) -> Self {
        Self {
            id: c.id.to_string(),
            name: c.name,
            key: c.key,
            value: c.value,
            remark: c.remark,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug)]
pub struct ConfigListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub key: Option<String>,
}

pub struct ConfigService {
    db: DatabaseCluster,
    config_repo: ConfigRepository,
    cache_namespace_repo: CacheNamespaceVersionRepository,
    authorization_cache: AuthorizationCache,
}

impl ConfigService {
    pub fn new(db: DatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
        Self {
            db,
            config_repo: ConfigRepository,
            cache_namespace_repo: CacheNamespaceVersionRepository,
            authorization_cache,
        }
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: ConfigListParams,
    ) -> AppResult<PageResult<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let filter = ConfigFilter {
            name: params.name.as_deref(),
            key: params.key.as_deref(),
        };
        let page = self
            .config_repo
            .find_by_page_filtered(&db, tenant_id, &params.page, &filter)
            .await?;
        let records = page.records.into_iter().map(ConfigVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 以稳定主键游标分批读取参数配置导出数据。
    pub async fn find_for_export(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        key: Option<&str>,
        maximum_records: usize,
    ) -> AppResult<Vec<ConfigVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let filter = ConfigFilter { name, key };
        let mut after_id = None;
        let mut records = Vec::new();
        loop {
            let batch = self
                .config_repo
                .find_for_export_after_id(&db, tenant_id, &filter, after_id, BATCH_SIZE)
                .await?;
            if batch.is_empty() {
                break;
            }
            after_id = batch.last().map(|config| config.id);
            records.extend(batch.into_iter().map(ConfigVo::from));
            if records.len() > maximum_records {
                return Err(AppError::Validation(format!(
                    "导出记录数超过 {maximum_records} 条上限"
                )));
            }
        }
        Ok(records)
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        Ok(self
            .config_repo
            .find_by_id(&db, tenant_id, id)
            .await?
            .map(ConfigVo::from))
    }

    pub async fn find_by_key(
        &self,
        actor: &ActorContext,
        key: &str,
    ) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.find_by_key_in_tenant(tenant_id, key, ReadConsistency::Eventual)
            .await
    }

    /// 读取认证完成前所需的一项租户配置。
    pub async fn find_public_value(&self, tenant_id: &str, key: &str) -> AppResult<Option<String>> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        Ok(self
            .find_by_key_in_tenant(tenant_id, key, ReadConsistency::Strong)
            .await?
            .map(|config| config.value))
    }

    async fn find_by_key_in_tenant(
        &self,
        tenant_id: &str,
        key: &str,
        consistency: ReadConsistency,
    ) -> AppResult<Option<ConfigVo>> {
        // 强一致性读取绕过缓存，避免认证前决策使用失效配置。普通读取先查 Redis；
        // version key 丢失时从主库恢复权威版本，绝不在 Redis 中猜测初始值。
        let cache_lookup =
            if consistency == ReadConsistency::Eventual && self.authorization_cache.is_enabled() {
                match self
                    .authorization_cache
                    .read_namespace_value(tenant_id, CONFIG_CACHE_NAMESPACE, key)
                    .await?
                {
                    Some(lookup) => Some(lookup),
                    None => {
                        let namespace_version = self
                            .cache_namespace_repo
                            .find_version(self.db.write(), tenant_id, CONFIG_CACHE_NAMESPACE)
                            .await?;
                        self.authorization_cache
                            .sync_namespace_version(
                                tenant_id,
                                CONFIG_CACHE_NAMESPACE,
                                namespace_version,
                            )
                            .await?;
                        Some(NamespaceCacheLookup {
                            namespace_version,
                            value: None,
                        })
                    }
                }
            } else {
                None
            };
        if let Some(json) = cache_lookup
            .as_ref()
            .and_then(|lookup| lookup.value.as_deref())
            && let Ok(cached) = serde_json::from_str::<ConfigVo>(json)
        {
            return Ok(Some(cached));
        }

        // 所有缓存未命中都回源主库；副本延迟不能把旧配置重新写回新命名空间。
        // 热命中在到达此处前已经返回，因此不会产生 SQL。
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let result = self
            .config_repo
            .find_by_key(&db, tenant_id, key)
            .await?
            .map(ConfigVo::from);

        if let (Some(cache_lookup), Some(vo)) = (cache_lookup, result.as_ref()) {
            let json = serde_json::to_string(vo)
                .map_err(|error| AppError::Internal(format!("序列化参数配置缓存失败: {error}")))?;
            self.authorization_cache
                .store_namespace_value(
                    tenant_id,
                    CONFIG_CACHE_NAMESPACE,
                    key,
                    cache_lookup.namespace_version,
                    &json,
                    CACHE_TTL_SECS,
                )
                .await?;
        }

        Ok(result)
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        name: &str,
        key: &str,
        value: &str,
        remark: Option<&str>,
    ) -> AppResult<ConfigVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        // 检查 key 是否已存在
        if self
            .config_repo
            .find_by_key(db, tenant_id, key)
            .await?
            .is_some()
        {
            return Err(AppError::Validation(format!("参数键名 '{}' 已存在", key)));
        }

        let mut new_config = config::Model {
            id: ryframe_utils::snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            remark: remark.map(|s| s.to_string()),
            del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_config.fill_on_insert(&FillContext::new())?;

        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let saved = self
            .config_repo
            .insert_in_transaction(&transaction, tenant_id, new_config)
            .await?;
        let namespace_version = self
            .authorization_cache
            .record_namespace_version_in_transaction(
                &transaction,
                tenant_id,
                CONFIG_CACHE_NAMESPACE,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn update(&self, actor: &ActorContext, id: i64, value: &str) -> AppResult<ConfigVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let cfg = self
            .config_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let mut cfg = self
            .config_repo
            .find_by_id_for_update(&transaction, tenant_id, cfg.id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        cfg.value = value.to_string();
        cfg.fill_on_update(&FillContext::new())?;

        let saved = self
            .config_repo
            .update_in_transaction(&transaction, tenant_id, cfg)
            .await?;
        let namespace_version = self
            .authorization_cache
            .record_namespace_version_in_transaction(
                &transaction,
                tenant_id,
                CONFIG_CACHE_NAMESPACE,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let cfg = self
            .config_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        self.config_repo
            .find_by_id_for_update(&transaction, tenant_id, cfg.id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        self.config_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        let namespace_version = self
            .authorization_cache
            .record_namespace_version_in_transaction(
                &transaction,
                tenant_id,
                CONFIG_CACHE_NAMESPACE,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(())
    }

    /// 递增独立配置命名空间版本，废弃当前租户的全部参数配置缓存。
    pub async fn clear_cache(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let namespace_version = self
            .authorization_cache
            .record_namespace_version_in_transaction(
                &transaction,
                tenant_id,
                CONFIG_CACHE_NAMESPACE,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use ryframe_db::{DatabaseCluster, entities::cache_namespace_version};
    use sea_orm::{DatabaseBackend, MockDatabase};

    use crate::{
        AuthorizationCacheBackend, AuthorizationCacheLookup, AuthorizationSnapshot,
        TenantCacheLookup,
    };

    use super::*;

    struct NamespaceBackend {
        lookup: Mutex<Option<NamespaceCacheLookup>>,
        reads: AtomicUsize,
        stores: AtomicUsize,
        mirrored_version: AtomicI64,
    }

    impl NamespaceBackend {
        fn returning(lookup: Option<NamespaceCacheLookup>) -> Self {
            Self {
                lookup: Mutex::new(lookup),
                reads: AtomicUsize::new(0),
                stores: AtomicUsize::new(0),
                mirrored_version: AtomicI64::new(-1),
            }
        }
    }

    #[async_trait]
    impl AuthorizationCacheBackend for NamespaceBackend {
        async fn lookup_snapshot(
            &self,
            _tenant_id: &str,
            _user_id: i64,
        ) -> Result<AuthorizationCacheLookup, String> {
            Err("本测试不读取授权快照".into())
        }

        async fn store_snapshot(&self, _snapshot: &AuthorizationSnapshot) -> Result<bool, String> {
            Ok(false)
        }

        async fn update_tenant_epoch(
            &self,
            _tenant_id: &str,
            _authorization_epoch: i32,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn update_user_version(
            &self,
            _tenant_id: &str,
            _user_id: i64,
            _authorization_version: i32,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn read_tenant_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
        ) -> Result<Option<TenantCacheLookup>, String> {
            Ok(None)
        }

        async fn store_tenant_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _authorization_epoch: i32,
            _value: &str,
            _ttl_secs: u64,
        ) -> Result<bool, String> {
            Ok(false)
        }

        async fn update_namespace_version(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            namespace_version: i64,
        ) -> Result<(), String> {
            self.mirrored_version
                .fetch_max(namespace_version, Ordering::SeqCst);
            Ok(())
        }

        async fn read_namespace_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _item: &str,
        ) -> Result<Option<NamespaceCacheLookup>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.lookup.lock().unwrap().clone())
        }

        async fn store_namespace_value(
            &self,
            _tenant_id: &str,
            _namespace: &str,
            _item: &str,
            _namespace_version: i64,
            _value: &str,
            _ttl_secs: u64,
        ) -> Result<bool, String> {
            self.stores.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    fn config_model(value: &str) -> config::Model {
        config::Model {
            id: 1,
            tenant_id: "system".into(),
            name: "界面主题".into(),
            key: "system.theme".into(),
            value: value.into(),
            remark: None,
            del_flag: config::Model::DEL_FLAG_NORMAL.into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn cached_value(value: &str) -> String {
        serde_json::to_string(&ConfigVo::from(config_model(value))).unwrap()
    }

    #[tokio::test]
    async fn hot_cache_hit_performs_zero_sql() {
        let backend = Arc::new(NamespaceBackend::returning(Some(NamespaceCacheLookup {
            namespace_version: 7,
            value: Some(cached_value("dark")),
        })));
        let primary = MockDatabase::new(DatabaseBackend::MySql).into_connection();
        let service = ConfigService::new(
            DatabaseCluster::single(primary),
            AuthorizationCache::from_backend(backend.clone(), false),
        );

        let result = service
            .find_by_key_in_tenant("system", "system.theme", ReadConsistency::Eventual)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.value, "dark");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 1);
        assert_eq!(backend.stores.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cache_miss_reads_primary_instead_of_replica() {
        let backend = Arc::new(NamespaceBackend::returning(Some(NamespaceCacheLookup {
            namespace_version: 8,
            value: None,
        })));
        let primary = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([vec![config_model("primary")]])
            .into_connection();
        let replica = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([vec![config_model("replica")]])
            .into_connection();
        let cluster = DatabaseCluster::with_sources_and_replica_slots(
            primary,
            [("replica".into(), Some(replica), true)],
            std::iter::empty(),
        );
        let service = ConfigService::new(
            cluster,
            AuthorizationCache::from_backend(backend.clone(), false),
        );

        let result = service
            .find_by_key_in_tenant("system", "system.theme", ReadConsistency::Eventual)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.value, "primary");
        assert_eq!(backend.stores.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_redis_version_is_restored_from_database() {
        let backend = Arc::new(NamespaceBackend::returning(None));
        let now = chrono::Utc::now();
        let primary = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([vec![cache_namespace_version::Model {
                tenant_id: "system".into(),
                namespace: CONFIG_CACHE_NAMESPACE.into(),
                version: 37,
                created_at: now,
                updated_at: now,
            }]])
            .append_query_results([vec![config_model("restored")]])
            .into_connection();
        let service = ConfigService::new(
            DatabaseCluster::single(primary),
            AuthorizationCache::from_backend(backend.clone(), false),
        );

        let result = service
            .find_by_key_in_tenant("system", "system.theme", ReadConsistency::Eventual)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.value, "restored");
        assert_eq!(backend.mirrored_version.load(Ordering::SeqCst), 37);
        assert_eq!(backend.stores.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn namespace_repair_accepts_duplicate_and_out_of_order_delivery() {
        let backend = Arc::new(NamespaceBackend::returning(None));
        let cache = AuthorizationCache::from_backend(backend.clone(), true);

        for version in [5, 5, 3, 7, 7] {
            cache
                .repair_namespace_version("tenant-a", CONFIG_CACHE_NAMESPACE, version)
                .await
                .unwrap();
        }

        assert_eq!(backend.mirrored_version.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn optional_disabled_cache_consumes_namespace_outbox_as_noop() {
        AuthorizationCache::disabled()
            .repair_namespace_version("tenant-a", CONFIG_CACHE_NAMESPACE, 9)
            .await
            .unwrap();
    }
}

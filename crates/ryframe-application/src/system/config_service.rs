use ryframe_adapters::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, ConfigFilter, ConfigRepository,
    ExportCursorWindow, TenantConfigTransferRepository, entities::config,
};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};
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
    pub portable: bool,
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
            portable: c.portable,
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
    db: ControlDatabaseCluster,
    config_repo: ConfigRepository,
    cache_namespace_repo: CacheNamespaceVersionRepository,
    authorization_cache: AuthorizationCache,
}

impl ConfigService {
    pub fn new(db: ControlDatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
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
        upper_id: i64,
        maximum_records: usize,
    ) -> AppResult<Vec<ConfigVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let filter = ConfigFilter { name, key };
        let mut after_id = None;
        let mut records = Vec::new();
        loop {
            let batch = self
                .config_repo
                .find_for_export_after_id(
                    &db,
                    tenant_id,
                    &filter,
                    ExportCursorWindow::new(after_id, upper_id, BATCH_SIZE),
                )
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
        ryframe_adapters::validate_explicit_tenant(tenant_id)?;
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
        self.create_with_portability(actor, name, key, value, remark, false)
            .await
    }

    /// 创建参数配置，并显式控制其是否允许进入租户配置包。
    pub async fn create_with_portability(
        &self,
        actor: &ActorContext,
        name: &str,
        key: &str,
        value: &str,
        remark: Option<&str>,
        portable: bool,
    ) -> AppResult<ConfigVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_portable_key(key, portable)?;
        let db = self.db.write();
        let mut new_config = config::Model {
            id: ryframe_utils::snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            portable,
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
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        if config::Entity::find()
            .filter(config::Column::TenantId.eq(tenant_id))
            .filter(config::Column::Key.eq(key))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .lock(sea_orm::sea_query::LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some()
        {
            return Err(AppError::Validation(format!("参数键名 '{key}' 已存在")));
        }
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
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn update(&self, actor: &ActorContext, id: i64, value: &str) -> AppResult<ConfigVo> {
        self.update_with_portability(actor, id, value, None).await
    }

    /// 更新参数值，并在指定时同步配置包迁移标记。
    pub async fn update_with_portability(
        &self,
        actor: &ActorContext,
        id: i64,
        value: &str,
        portable: Option<bool>,
    ) -> AppResult<ConfigVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut cfg = self
            .config_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        let portable = portable.unwrap_or(cfg.portable);
        validate_portable_key(&cfg.key, portable)?;
        cfg.value = value.to_string();
        cfg.portable = portable;
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
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
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
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.config_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
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
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
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

fn validate_portable_key(key: &str, portable: bool) -> AppResult<()> {
    if !portable {
        return Ok(());
    }
    if super::tenant_config_package::is_sensitive_config_key(key) {
        return Err(AppError::Validation("敏感参数禁止加入租户配置包".into()));
    }
    Ok(())
}

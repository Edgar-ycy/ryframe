use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use serde::{Deserialize, Serialize};

use crate::{
    AuthorizationCache, NamespaceCacheLookup,
    ports::system::{ConfigFilter, ConfigPersistencePort, ConfigRecord},
};

const CACHE_TTL_SECS: u64 = 3600;
const CONFIG_CACHE_NAMESPACE: &str = "config";

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
    pub id: String,
    pub name: String,
    pub key: String,
    pub value: String,
    pub portable: bool,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ConfigRecord> for ConfigVo {
    fn from(config: ConfigRecord) -> Self {
        Self {
            id: config.id.to_string(),
            name: config.name,
            key: config.key,
            value: config.value,
            portable: config.portable,
            remark: config.remark,
            created_at: config.created_at,
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
    persistence: Arc<dyn ConfigPersistencePort>,
    authorization_cache: AuthorizationCache,
}

impl ConfigService {
    pub fn new(
        persistence: Arc<dyn ConfigPersistencePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            persistence,
            authorization_cache,
        }
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: ConfigListParams,
    ) -> AppResult<PageResult<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let filter = ConfigFilter {
            name: params.name.as_deref(),
            key: params.key.as_deref(),
        };
        let page = self
            .persistence
            .find_by_page(tenant_id, params.page, filter)
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(ConfigVo::from).collect(),
            page.total,
            &params.page,
        ))
    }

    /// 按稳定主键窗口读取一批参数配置导出数据。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        key: Option<&str>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_export_batch(tenant_id, ConfigFilter { name, key }, window)
            .await?
            .into_iter()
            .map(ConfigVo::from)
            .collect())
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_by_id(tenant_id, id)
            .await?
            .map(ConfigVo::from))
    }

    pub async fn find_by_key(
        &self,
        actor: &ActorContext,
        key: &str,
    ) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.find_by_key_in_tenant(tenant_id, key, true).await
    }

    /// 读取认证完成前所需的一项租户配置。
    pub async fn find_public_value(&self, tenant_id: &str, key: &str) -> AppResult<Option<String>> {
        crate::enforce_tenant_scope(tenant_id)?;
        Ok(self
            .find_by_key_in_tenant(tenant_id, key, false)
            .await?
            .map(|config| config.value))
    }

    async fn find_by_key_in_tenant(
        &self,
        tenant_id: &str,
        key: &str,
        allow_cache: bool,
    ) -> AppResult<Option<ConfigVo>> {
        let cache_lookup = if allow_cache && self.authorization_cache.is_enabled() {
            match self
                .authorization_cache
                .read_namespace_value(tenant_id, CONFIG_CACHE_NAMESPACE, key)
                .await?
            {
                Some(lookup) => Some(lookup),
                None => {
                    let namespace_version = self
                        .persistence
                        .find_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE)
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

        let result = self
            .persistence
            .find_by_key(tenant_id, key)
            .await?
            .map(ConfigVo::from);
        if let (Some(cache_lookup), Some(config)) = (cache_lookup, result.as_ref()) {
            let json = serde_json::to_string(config)
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
        let now = Utc::now();
        let record = ConfigRecord {
            id: crate::next_id()?,
            name: name.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
            portable,
            remark: remark.map(str::to_owned),
            created_at: now,
            updated_at: now,
        };
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        if transaction
            .find_by_key_for_update(tenant_id, key)
            .await?
            .is_some()
        {
            return Err(AppError::Validation(format!("参数键名 '{key}' 已存在")));
        }
        let saved = transaction.insert(tenant_id, record).await?;
        let namespace_version = transaction
            .record_namespace_change(tenant_id, CONFIG_CACHE_NAMESPACE)
            .await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
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
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut config = transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        let portable = portable.unwrap_or(config.portable);
        validate_portable_key(&config.key, portable)?;
        config.value = value.to_owned();
        config.portable = portable;
        config.updated_at = Utc::now();
        let saved = transaction.update(tenant_id, config).await?;
        let namespace_version = transaction
            .record_namespace_change(tenant_id, CONFIG_CACHE_NAMESPACE)
            .await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;
        transaction.delete(tenant_id, id).await?;
        let namespace_version = transaction
            .record_namespace_change(tenant_id, CONFIG_CACHE_NAMESPACE)
            .await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await
    }

    /// 递增独立配置命名空间版本，废弃当前租户的全部参数配置缓存。
    pub async fn clear_cache(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let namespace_version = transaction
            .record_namespace_change(tenant_id, CONFIG_CACHE_NAMESPACE)
            .await?;
        transaction.commit().await?;
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
    if super::tenant::config_package::is_sensitive_config_key(key) {
        return Err(AppError::Validation("敏感参数禁止加入租户配置包".into()));
    }
    Ok(())
}

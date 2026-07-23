use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::DatabaseCluster;
use ryframe_db::{ConfigFilter, ConfigRepository, entities::config};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 参数配置缓存 Redis key 前缀
const CONFIG_CACHE_KEY_PREFIX: &str = "sys_config:key:";
/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

fn config_cache_key(tenant_id: &str, key: &str) -> String {
    format!("{CONFIG_CACHE_KEY_PREFIX}{tenant_id}:{key}")
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    pub page: PageQuery,
    pub name: Option<String>,
    pub key: Option<String>,
}

pub struct ConfigService {
    db: DatabaseCluster,
    config_repo: LoggedRepo<ConfigRepository>,
    redis: Option<ryframe_core::RedisClient>,
}

impl ConfigService {
    pub fn new(db: DatabaseCluster, redis: Option<ryframe_core::RedisClient>) -> Self {
        Self {
            db,
            config_repo: LoggedRepo::new(ConfigRepository),
            redis,
        }
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: ConfigListParams,
    ) -> AppResult<PageResult<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        let filter = ConfigFilter {
            name: params.name.as_deref(),
            key: params.key.as_deref(),
        };
        let page = self
            .config_repo
            .find_by_page_filtered(db, tenant_id, &params.page, &filter)
            .await?;
        let records = page.records.into_iter().map(ConfigVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        Ok(self
            .config_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .map(ConfigVo::from))
    }

    pub async fn find_by_key(
        &self,
        actor: &ActorContext,
        key: &str,
    ) -> AppResult<Option<ConfigVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.find_by_key_in_tenant(tenant_id, key).await
    }

    /// Read one tenant configuration required before authentication completes.
    pub async fn find_public_value(&self, tenant_id: &str, key: &str) -> AppResult<Option<String>> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        Ok(self
            .find_by_key_in_tenant(tenant_id, key)
            .await?
            .map(|config| config.value))
    }

    async fn find_by_key_in_tenant(
        &self,
        tenant_id: &str,
        key: &str,
    ) -> AppResult<Option<ConfigVo>> {
        let db = self.db.read();
        // 尝试从 Redis 缓存读取
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(&config_cache_key(tenant_id, key)).await
            && let Ok(cached) = serde_json::from_str::<ConfigVo>(&json)
        {
            return Ok(Some(cached));
        }

        let result = self
            .config_repo
            .find_by_key(db, tenant_id, key)
            .await?
            .map(ConfigVo::from);

        // 写入缓存
        if let Some(ref redis) = self.redis
            && let Some(ref vo) = result
            && let Ok(json) = serde_json::to_string(vo)
        {
            let cache_key = config_cache_key(tenant_id, key);
            if let Err(error) = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await {
                tracing::warn!(tenant_id, key, %error, "failed to cache config value");
            }
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
            id: ryframe_common::utils::snowflake::try_next_snowflake_id()?,
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

        let saved = self.config_repo.insert(db, tenant_id, new_config).await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn update(&self, actor: &ActorContext, id: i64, value: &str) -> AppResult<ConfigVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut cfg = self
            .config_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        let key = cfg.key.clone();
        cfg.value = value.to_string();
        cfg.fill_on_update(&FillContext::new())?;

        let saved = self.config_repo.update(db, tenant_id, cfg).await?;
        let vo = ConfigVo::from(saved);

        // 更新缓存
        self.invalidate_config_cache(tenant_id, &key).await;

        Ok(vo)
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let cfg = self
            .config_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        let key = cfg.key.clone();
        self.config_repo.delete(db, tenant_id, id).await?;

        // 更新缓存
        self.invalidate_config_cache(tenant_id, &key).await;

        Ok(())
    }

    /// 查询所有参数（用于导出）
    pub async fn find_all(
        &self,
        actor: &ActorContext,
        mut params: ConfigListParams,
    ) -> AppResult<Vec<ConfigVo>> {
        params.page = PageQuery::all_records();
        Ok(self.find_by_page(actor, params).await?.records)
    }

    /// 清除当前租户的全部参数配置缓存。
    pub async fn clear_cache(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let Some(redis) = &self.redis else {
            return Ok(0);
        };
        let pattern = format!("{CONFIG_CACHE_KEY_PREFIX}{tenant_id}:*");
        redis.delete_by_pattern(&pattern).await.map_err(|error| {
            tracing::error!(tenant_id, %error, "failed to clear config cache");
            AppError::Internal("清除参数缓存失败".into())
        })
    }

    /// 清除参数配置缓存
    async fn invalidate_config_cache(&self, tenant_id: &str, key: &str) {
        if let Some(redis) = &self.redis
            && let Err(error) = redis.del(config_cache_key(tenant_id, key)).await
        {
            tracing::warn!(tenant_id, key, %error, "failed to invalidate config cache");
        }
    }
}

use ryframe_common::{AppError, AppResult};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::{ConfigRepository, entities::config};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

/// 参数配置缓存 Redis key 前缀
const CONFIG_CACHE_KEY_PREFIX: &str = "sys_config:key:";
/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigVo {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub value: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<config::Model> for ConfigVo {
    fn from(c: config::Model) -> Self {
        Self {
            id: c.id,
            name: c.name,
            key: c.key,
            value: c.value,
            remark: c.remark,
            created_at: c.created_at,
        }
    }
}

pub struct ConfigServiceImpl {
    pub config_repo: LoggedRepo<ConfigRepository>,
    pub redis: Option<ryframe_core::RedisClient>,
}

impl ConfigServiceImpl {
    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<ConfigVo>> {
        let page = self.config_repo.find_by_page(db, query.clone()).await?;
        let records = page.records.into_iter().map(ConfigVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<ConfigVo>> {
        Ok(self
            .config_repo
            .find_by_id(db, id)
            .await?
            .map(ConfigVo::from))
    }

    pub async fn find_by_key(
        &self,
        db: &DatabaseConnection,
        key: &str,
    ) -> AppResult<Option<ConfigVo>> {
        // 尝试从 Redis 缓存读取
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis
                .get(&format!("{}{}", CONFIG_CACHE_KEY_PREFIX, key))
                .await
            && let Ok(cached) = serde_json::from_str::<ConfigVo>(&json)
        {
            return Ok(Some(cached));
        }

        let result = self
            .config_repo
            .find_by_key(db, key)
            .await?
            .map(ConfigVo::from);

        // 写入缓存
        if let Some(ref redis) = self.redis
            && let Some(ref vo) = result
            && let Ok(json) = serde_json::to_string(vo)
        {
            let cache_key = format!("{}{}", CONFIG_CACHE_KEY_PREFIX, key);
            let _ = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await;
        }

        Ok(result)
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        key: &str,
        value: &str,
        remark: Option<&str>,
    ) -> AppResult<ConfigVo> {
        // 检查 key 是否已存在
        if self.config_repo.find_by_key(db, key).await?.is_some() {
            return Err(AppError::Validation(format!("参数键名 '{}' 已存在", key)));
        }

        let mut new_config = config::Model {
            id: ryframe_common::utils::snowflake::next_snowflake_id(),
            name: name.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            remark: remark.map(|s| s.to_string()),
            del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_config.fill_on_insert(&FillContext::new());

        let saved = self.config_repo.insert(db, new_config).await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        value: &str,
    ) -> AppResult<ConfigVo> {
        let mut cfg = self
            .config_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        let key = cfg.key.clone();
        tracing::info!(
            "[ConfigUpdate] id={}, key={}, old_value={}, input_value={}",
            id,
            key,
            cfg.value,
            value
        );
        cfg.value = value.to_string();
        tracing::info!("[ConfigUpdate] after set: cfg.value={}", cfg.value);
        cfg.fill_on_update(&FillContext::new());

        let saved = self.config_repo.update(db, cfg).await?;
        let vo = ConfigVo::from(saved.clone());
        tracing::info!(
            "[ConfigUpdate] DB saved.value={}, vo.value={}",
            saved.value,
            vo.value
        );

        // 便新缓存
        self.invalidate_config_cache(&key).await;

        Ok(vo)
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let cfg = self
            .config_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        let key = cfg.key.clone();
        self.config_repo.delete(db, id).await?;

        // 便新缓存
        self.invalidate_config_cache(&key).await;

        Ok(())
    }

    /// 查询所有参数（用于导出）
    pub async fn find_all(&self, db: &DatabaseConnection) -> AppResult<Vec<ConfigVo>> {
        let query = PageQuery {
            page: 1,
            page_size: 10000,
        };
        let page = self.config_repo.find_by_page(db, query).await?;
        Ok(page.records.into_iter().map(ConfigVo::from).collect())
    }

    /// 清除参数配置缓存
    async fn invalidate_config_cache(&self, key: &str) {
        if let Some(ref redis) = self.redis {
            let cache_key = format!("{}{}", CONFIG_CACHE_KEY_PREFIX, key);
            let _ = redis.del(&cache_key).await;
        }
    }
}

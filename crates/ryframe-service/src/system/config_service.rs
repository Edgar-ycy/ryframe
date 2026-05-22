use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::entities::config;
use ryframe_db::ConfigRepository;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_core::Repository;

#[derive(Debug, Serialize)]
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
    pub config_repo: ConfigRepository,
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

    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<ConfigVo>> {
        Ok(self.config_repo.find_by_id(db, id).await?.map(ConfigVo::from))
    }

    pub async fn find_by_key(&self, db: &DatabaseConnection, key: &str) -> AppResult<Option<ConfigVo>> {
        Ok(self.config_repo.find_by_key(db, key).await?.map(ConfigVo::from))
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

        let now = chrono::Utc::now();
        let new_config = config::Model {
            id: ryframe_common::utils::snowflake::next_snowflake_id(),
            name: name.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            remark: remark.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
        };

        let saved = self.config_repo.insert(db, new_config).await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        value: &str,
    ) -> AppResult<ConfigVo> {
        let mut cfg = self.config_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        cfg.value = value.to_string();
        cfg.updated_at = chrono::Utc::now();

        let saved = self.config_repo.update(db, cfg).await?;
        Ok(ConfigVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.config_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))?;

        self.config_repo.delete(db, id).await
    }
}

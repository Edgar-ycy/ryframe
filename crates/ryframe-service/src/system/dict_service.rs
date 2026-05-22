use ryframe_common::{AppError, AppResult};

use ryframe_db::entities::{dict_data, dict_type};
use ryframe_db::{DictDataRepository, DictTypeRepository};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;

#[derive(Debug, Serialize)]
pub struct DictTypeVo {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
}

impl From<dict_type::Model> for DictTypeVo {
    fn from(t: dict_type::Model) -> Self {
        Self { id: t.id, name: t.name, code: t.code, status: t.status, remark: t.remark }
    }
}

#[derive(Debug, Serialize)]
pub struct DictDataVo {
    pub id: i64,
    pub type_code: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
}

impl From<dict_data::Model> for DictDataVo {
    fn from(d: dict_data::Model) -> Self {
        Self {
            id: d.id, type_code: d.type_code, label: d.label,
            value: d.value, sort: d.sort, status: d.status, css_class: d.css_class,
        }
    }
}

pub struct DictServiceImpl {
    pub dict_type_repo: DictTypeRepository,
    pub dict_data_repo: DictDataRepository,
}

impl DictServiceImpl {
    // --- 字典类型 ---

    pub async fn find_types(&self, db: &DatabaseConnection) -> AppResult<Vec<DictTypeVo>> {
        let types = self.dict_type_repo.find_all(db).await?;
        Ok(types.into_iter().map(DictTypeVo::from).collect())
    }

    pub async fn create_type(
        &self, db: &DatabaseConnection, name: &str, code: &str,
    ) -> AppResult<DictTypeVo> {
        if self.dict_type_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("字典类型编码已存在".into()));
        }
        let now = chrono::Utc::now();
        let new_type = dict_type::Model {
            id: snowflake::next_snowflake_id(), name: name.to_string(), code: code.to_string(),
            status: dict_type::Model::STATUS_NORMAL.to_string(), remark: None,
            created_at: now, updated_at: now,
        };
        Ok(DictTypeVo::from(self.dict_type_repo.insert(db, new_type).await?))
    }

    pub async fn update_type(
        &self, db: &DatabaseConnection, id: i64, name: &str, status: String,
    ) -> AppResult<DictTypeVo> {
        let mut t = self.dict_type_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        t.name = name.to_string();
        t.status = status;
        t.updated_at = chrono::Utc::now();
        Ok(DictTypeVo::from(self.dict_type_repo.update(db, t).await?))
    }

    pub async fn delete_type(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.dict_type_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        self.dict_type_repo.delete(db, id).await
    }

    // --- 字典数据 ---

    pub async fn find_data_by_type(
        &self, db: &DatabaseConnection, type_code: &str,
    ) -> AppResult<Vec<DictDataVo>> {
        let data = self.dict_data_repo.find_by_type_code(db, type_code).await?;
        Ok(data.into_iter().map(DictDataVo::from).collect())
    }

    pub async fn create_data(
        &self, db: &DatabaseConnection, type_code: &str, label: &str, value: &str, sort: i32,
    ) -> AppResult<DictDataVo> {
        let now = chrono::Utc::now();
        let new_data = dict_data::Model {
            id: snowflake::next_snowflake_id(), type_code: type_code.to_string(), label: label.to_string(),
            value: value.to_string(), sort, status: dict_data::Model::STATUS_NORMAL.to_string(),
            css_class: None, remark: None, created_at: now, updated_at: now,
        };
        Ok(DictDataVo::from(self.dict_data_repo.insert(db, new_data).await?))
    }

    pub async fn update_data(
        &self, db: &DatabaseConnection, id: i64,
        label: &str, value: &str, sort: i32, status: String,
    ) -> AppResult<DictDataVo> {
        let mut d = self.dict_data_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        d.label = label.to_string();
        d.value = value.to_string();
        d.sort = sort;
        d.status = status;
        d.updated_at = chrono::Utc::now();
        Ok(DictDataVo::from(self.dict_data_repo.update(db, d).await?))
    }

    pub async fn delete_data(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.dict_data_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        self.dict_data_repo.delete(db, id).await
    }
}

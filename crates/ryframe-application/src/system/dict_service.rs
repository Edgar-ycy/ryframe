use std::{future::Future, pin::Pin, sync::Arc};

use chrono::Utc;
use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use serde::{Deserialize, Serialize};

use crate::ports::system::{DictDataRecord, DictPersistencePort, DictTypeFilter, DictTypeRecord};

const DICT_CACHE_KEY_PREFIX: &str = "sys_dict:data:";
const CACHE_TTL_SECS: u64 = 3600;

pub type DictCacheStoreFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

pub trait DictCacheStore: Send + Sync {
    fn get<'a>(&'a self, key: &'a str) -> DictCacheStoreFuture<'a, Option<String>>;

    fn put(&self, key: String, value: String, ttl_secs: u64) -> DictCacheStoreFuture<'_, ()>;

    fn remove(&self, key: String) -> DictCacheStoreFuture<'_, ()>;
}

fn dict_cache_key(tenant_id: &str, type_code: &str) -> String {
    format!("{DICT_CACHE_KEY_PREFIX}{tenant_id}:{type_code}")
}

#[derive(Debug, Serialize)]
pub struct DictTypeVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<DictTypeRecord> for DictTypeVo {
    fn from(dict_type: DictTypeRecord) -> Self {
        Self {
            id: dict_type.id.to_string(),
            name: dict_type.name,
            code: dict_type.code,
            status: dict_type.status,
            remark: dict_type.remark,
            created_at: dict_type.created_at,
        }
    }
}

#[derive(Debug)]
pub struct DictTypeListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DictDataVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
    pub id: String,
    pub type_code: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
}

impl From<DictDataRecord> for DictDataVo {
    fn from(data: DictDataRecord) -> Self {
        Self {
            id: data.id.to_string(),
            type_code: data.type_code,
            label: data.label,
            value: data.value,
            sort: data.sort,
            status: data.status,
            css_class: data.css_class,
        }
    }
}

pub struct DictService {
    persistence: Arc<dyn DictPersistencePort>,
    cache: Option<Arc<dyn DictCacheStore>>,
}

impl DictService {
    pub fn new(
        persistence: Arc<dyn DictPersistencePort>,
        cache: Option<Arc<dyn DictCacheStore>>,
    ) -> Self {
        Self { persistence, cache }
    }

    pub async fn find_types_by_page(
        &self,
        actor: &ActorContext,
        params: DictTypeListParams,
    ) -> AppResult<PageResult<DictTypeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let page = self
            .persistence
            .find_types_by_page(
                tenant_id,
                params.page,
                DictTypeFilter {
                    name: params.name.as_deref(),
                    code: params.code.as_deref(),
                    status: params.status.as_deref(),
                },
            )
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(DictTypeVo::from).collect(),
            page.total,
            &params.page,
        ))
    }

    /// 按稳定主键窗口读取一批字典类型导出数据。
    pub(crate) async fn find_type_export_batch(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<DictTypeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_type_export_batch(tenant_id, DictTypeFilter { name, code, status }, window)
            .await?
            .into_iter()
            .map(DictTypeVo::from)
            .collect())
    }

    pub async fn create_type(
        &self,
        actor: &ActorContext,
        name: &str,
        code: &str,
    ) -> AppResult<DictTypeVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = Utc::now();
        let record = DictTypeRecord {
            id: crate::next_id()?,
            name: name.to_owned(),
            code: code.to_owned(),
            status: "1".into(),
            remark: None,
            created_at: now,
            updated_at: now,
        };
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        if transaction
            .find_type_by_code_for_update(tenant_id, code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("字典类型编码已存在".into()));
        }
        let saved = transaction.insert_type(tenant_id, record).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        Ok(DictTypeVo::from(saved))
    }

    pub async fn update_type(
        &self,
        actor: &ActorContext,
        id: i64,
        name: &str,
        status: String,
    ) -> AppResult<DictTypeVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut dict_type = transaction
            .find_type_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        dict_type.name = name.to_owned();
        dict_type.status = status;
        dict_type.updated_at = Utc::now();
        let saved = transaction.update_type(tenant_id, dict_type).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        Ok(DictTypeVo::from(saved))
    }

    pub async fn delete_type(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_type_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        transaction.delete_type(tenant_id, id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await
    }

    pub async fn find_data_by_type(
        &self,
        actor: &ActorContext,
        type_code: &str,
    ) -> AppResult<Vec<DictDataVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if let Some(cache) = &self.cache
            && let Ok(Some(json)) = cache.get(&dict_cache_key(tenant_id, type_code)).await
            && let Ok(cached) = serde_json::from_str::<Vec<DictDataVo>>(&json)
        {
            return Ok(cached);
        }

        let values = self
            .persistence
            .find_data_by_type(tenant_id, type_code)
            .await?
            .into_iter()
            .map(DictDataVo::from)
            .collect::<Vec<_>>();
        if let Some(cache) = &self.cache {
            let cache_key = dict_cache_key(tenant_id, type_code);
            if let Ok(json) = serde_json::to_string(&values)
                && let Err(error) = cache.put(cache_key, json, CACHE_TTL_SECS).await
            {
                tracing::warn!(tenant_id, type_code, %error, "写入字典缓存失败");
            }
        }
        Ok(values)
    }

    pub async fn create_data(
        &self,
        actor: &ActorContext,
        type_code: &str,
        label: &str,
        value: &str,
        sort: i32,
    ) -> AppResult<DictDataVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = Utc::now();
        let record = DictDataRecord {
            id: crate::next_id()?,
            type_code: type_code.to_owned(),
            label: label.to_owned(),
            value: value.to_owned(),
            sort,
            status: "1".into(),
            css_class: None,
            remark: None,
            created_at: now,
            updated_at: now,
        };
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_type_by_code_for_update(tenant_id, type_code)
            .await?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        let saved = transaction.insert_data(tenant_id, record).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        let value = DictDataVo::from(saved);
        self.invalidate_dict_cache(tenant_id, &value.type_code)
            .await;
        Ok(value)
    }

    pub async fn update_data(
        &self,
        actor: &ActorContext,
        id: i64,
        label: &str,
        value: &str,
        sort: i32,
        status: String,
    ) -> AppResult<DictDataVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut data = transaction
            .find_data_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        data.label = label.to_owned();
        data.value = value.to_owned();
        data.sort = sort;
        data.status = status;
        data.updated_at = Utc::now();
        let saved = transaction.update_data(tenant_id, data).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        let value = DictDataVo::from(saved);
        self.invalidate_dict_cache(tenant_id, &value.type_code)
            .await;
        Ok(value)
    }

    pub async fn delete_data(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let data = transaction
            .find_data_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        transaction.delete_data(tenant_id, id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.invalidate_dict_cache(tenant_id, &data.type_code).await;
        Ok(())
    }

    async fn invalidate_dict_cache(&self, tenant_id: &str, type_code: &str) {
        if let Some(cache) = &self.cache
            && let Err(error) = cache.remove(dict_cache_key(tenant_id, type_code)).await
        {
            tracing::warn!(tenant_id, type_code, %error, "删除字典缓存失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_cache_key_is_tenant_scoped() {
        assert_eq!(
            dict_cache_key("tenant-a", "sys.status"),
            "sys_dict:data:tenant-a:sys.status"
        );
        assert_ne!(
            dict_cache_key("tenant-a", "sys.status"),
            dict_cache_key("tenant-b", "sys.status")
        );
    }
}

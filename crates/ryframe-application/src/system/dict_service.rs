use ryframe_adapters::{
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{
    DictDataRepository, DictTypeFilter, DictTypeRepository, ExportCursorWindow,
    TenantConfigTransferRepository,
    entities::{dict_data, dict_type},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};
use serde::{Deserialize, Serialize};

/// 字典缓存 Redis key 前缀
const DICT_CACHE_KEY_PREFIX: &str = "sys_dict:data:";
/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

fn dict_cache_key(tenant_id: &str, type_code: &str) -> String {
    format!("{DICT_CACHE_KEY_PREFIX}{tenant_id}:{type_code}")
}

#[derive(Debug, Serialize)]
pub struct DictTypeVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<dict_type::Model> for DictTypeVo {
    fn from(t: dict_type::Model) -> Self {
        Self {
            id: t.id.to_string(),
            name: t.name,
            code: t.code,
            status: t.status,
            remark: t.remark,
            created_at: t.created_at,
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
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
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
            id: d.id.to_string(),
            type_code: d.type_code,
            label: d.label,
            value: d.value,
            sort: d.sort,
            status: d.status,
            css_class: d.css_class,
        }
    }
}

pub struct DictService {
    db: ControlDatabaseCluster,
    dict_type_repo: DictTypeRepository,
    dict_data_repo: DictDataRepository,
    redis: Option<ryframe_adapters::RedisClient>,
}

impl DictService {
    pub fn new(db: ControlDatabaseCluster, redis: Option<ryframe_adapters::RedisClient>) -> Self {
        Self {
            db,
            dict_type_repo: DictTypeRepository,
            dict_data_repo: DictDataRepository,
            redis,
        }
    }

    // --- 字典类型 ---

    /// 分页查询字典类型
    pub async fn find_types_by_page(
        &self,
        actor: &ActorContext,
        params: DictTypeListParams,
    ) -> AppResult<PageResult<DictTypeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let filter = DictTypeFilter {
            name: params.name.as_deref(),
            code: params.code.as_deref(),
            status: params.status.as_deref(),
        };
        let page = self
            .dict_type_repo
            .find_by_page_filtered(&db, tenant_id, &params.page, &filter)
            .await?;
        let records = page.records.into_iter().map(DictTypeVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 以稳定主键游标分批读取字典类型导出数据。
    pub async fn find_types_for_export(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        upper_id: i64,
        maximum_records: usize,
    ) -> AppResult<Vec<DictTypeVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let filter = DictTypeFilter { name, code, status };
        let mut after_id = None;
        let mut records = Vec::new();
        loop {
            let batch = self
                .dict_type_repo
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
            after_id = batch.last().map(|dict_type| dict_type.id);
            records.extend(batch.into_iter().map(DictTypeVo::from));
            if records.len() > maximum_records {
                return Err(AppError::Validation(format!(
                    "导出记录数超过 {maximum_records} 条上限"
                )));
            }
        }
        Ok(records)
    }

    pub async fn create_type(
        &self,
        actor: &ActorContext,
        name: &str,
        code: &str,
    ) -> AppResult<DictTypeVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut new_type = dict_type::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            code: code.to_string(),
            status: dict_type::Model::STATUS_NORMAL.to_string(),
            remark: None,
            del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_type.fill_on_insert(&FillContext::new())?;
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        if dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::Code.eq(code))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .is_some()
        {
            return Err(AppError::Conflict("字典类型编码已存在".into()));
        }
        let saved = self
            .dict_type_repo
            .insert_in_transaction(&transaction, tenant_id, new_type)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
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
        let db = self.db.write();
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut t = dict_type::Entity::find_by_id(id)
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        t.name = name.to_string();
        t.status = status;
        t.fill_on_update(&FillContext::new())?;
        let saved = self
            .dict_type_repo
            .update_in_transaction(&transaction, tenant_id, t)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(DictTypeVo::from(saved))
    }

    pub async fn delete_type(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        dict_type::Entity::find_by_id(id)
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        self.dict_type_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await
    }

    // --- 字典数据 ---

    pub async fn find_data_by_type(
        &self,
        actor: &ActorContext,
        type_code: &str,
    ) -> AppResult<Vec<DictDataVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        // 尝试从 Redis 缓存读取
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(&dict_cache_key(tenant_id, type_code)).await
            && let Ok(cached) = serde_json::from_str::<Vec<DictDataVo>>(&json)
        {
            return Ok(cached);
        }

        // 仅在缓存未命中后选择数据库节点，避免把缓存命中误计为数据库读取。
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let data = self
            .dict_data_repo
            .find_by_type_code(&db, tenant_id, type_code)
            .await?;
        let vos: Vec<DictDataVo> = data.into_iter().map(DictDataVo::from).collect();

        // 写入缓存
        if let Some(ref redis) = self.redis {
            let cache_key = dict_cache_key(tenant_id, type_code);
            if let Ok(json) = serde_json::to_string(&vos)
                && let Err(error) = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await
            {
                tracing::warn!(tenant_id, type_code, %error, "failed to cache dictionary data");
            }
        }

        Ok(vos)
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
        let db = self.db.write();
        let mut new_data = dict_data::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            type_code: type_code.to_string(),
            label: label.to_string(),
            value: value.to_string(),
            sort,
            status: dict_data::Model::STATUS_NORMAL.to_string(),
            css_class: None,
            remark: None,
            del_flag: dict_data::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_data.fill_on_insert(&FillContext::new())?;
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::Code.eq(type_code))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("字典类型不存在".into()))?;
        let saved = self
            .dict_data_repo
            .insert_in_transaction(&transaction, tenant_id, new_data)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        let vo = DictDataVo::from(saved);

        // 更新缓存
        self.invalidate_dict_cache(tenant_id, type_code).await;

        Ok(vo)
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
        let db = self.db.write();
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut d = dict_data::Entity::find_by_id(id)
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        let type_code = d.type_code.clone();
        d.label = label.to_string();
        d.value = value.to_string();
        d.sort = sort;
        d.status = status;
        d.fill_on_update(&FillContext::new())?;
        let saved = self
            .dict_data_repo
            .update_in_transaction(&transaction, tenant_id, d)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        let vo = DictDataVo::from(saved);

        // 更新缓存
        self.invalidate_dict_cache(tenant_id, &type_code).await;

        Ok(vo)
    }

    pub async fn delete_data(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db.begin().await.map_err(database_error)?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let d = dict_data::Entity::find_by_id(id)
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("字典数据不存在".into()))?;
        let type_code = d.type_code.clone();
        self.dict_data_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;

        // 更新缓存
        self.invalidate_dict_cache(tenant_id, &type_code).await;

        Ok(())
    }

    /// 清除字典类型缓存
    async fn invalidate_dict_cache(&self, tenant_id: &str, type_code: &str) {
        if let Some(redis) = &self.redis
            && let Err(error) = redis.del(dict_cache_key(tenant_id, type_code)).await
        {
            tracing::warn!(tenant_id, type_code, %error, "failed to invalidate dictionary cache");
        }
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

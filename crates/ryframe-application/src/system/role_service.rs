use ryframe_adapters::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{
    ExportCursorWindow, PermissionRepository, RoleFilter, RoleRepository,
    TenantConfigTransferRepository, TenantRepository, entities::role,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};
use serde::Serialize;

use crate::AuthorizationCache;

use super::{OptionItem, OptionList, ProductService};

fn first_missing_id<T, F>(requested_ids: &[i64], existing: &[T], id: F) -> Option<i64>
where
    F: Fn(&T) -> i64,
{
    let existing_ids = existing
        .iter()
        .map(id)
        .collect::<std::collections::HashSet<_>>();
    requested_ids
        .iter()
        .copied()
        .find(|requested_id| !existing_ids.contains(requested_id))
}

fn normalize_ids(ids: &mut Vec<i64>) {
    ids.sort_unstable();
    ids.dedup();
}

#[derive(Debug, Serialize)]
pub struct RoleVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 自定义数据权限的部门ID列表（仅查询详情时填充）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dept_ids: Option<Vec<String>>,
}

impl From<role::Model> for RoleVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id.to_string(),
            name: r.name,
            code: r.code,
            is_super: r.is_super,
            data_scope: r.data_scope,
            status: r.status,
            sort: r.sort,
            remark: r.remark,
            created_at: r.created_at,
            dept_ids: None,
        }
    }
}

#[derive(Debug)]
pub struct RoleListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

pub struct RoleService {
    db: ControlDatabaseCluster,
    role_repo: RoleRepository,
    perm_repo: PermissionRepository,
    product_service: Arc<ProductService>,
    authorization_cache: AuthorizationCache,
}

impl RoleService {
    pub fn new(
        db: ControlDatabaseCluster,
        authorization_cache: AuthorizationCache,
        product_service: Arc<ProductService>,
    ) -> Self {
        Self {
            db,
            role_repo: RoleRepository,
            perm_repo: PermissionRepository,
            product_service,
            authorization_cache,
        }
    }

    fn validate_data_scope(data_scope: &str) -> AppResult<()> {
        if matches!(data_scope, "1" | "2" | "3" | "4" | "5") {
            Ok(())
        } else {
            Err(AppError::Validation("无效的数据范围值".into()))
        }
    }

    fn validate_status(status: &str) -> AppResult<()> {
        if matches!(
            status,
            role::Model::STATUS_NORMAL | role::Model::STATUS_DISABLED
        ) {
            Ok(())
        } else {
            Err(AppError::Validation("无效的角色状态".into()))
        }
    }

    fn ensure_super_role_remains(available: usize, becoming_unavailable: usize) -> AppResult<()> {
        if becoming_unavailable > 0 && available <= becoming_unavailable {
            return Err(AppError::Conflict(
                "系统必须保留至少一个可用的超级管理员角色".into(),
            ));
        }
        Ok(())
    }

    pub async fn get_role_model(&self, actor: &ActorContext, id: i64) -> AppResult<role::Model> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.role_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: RoleListParams,
    ) -> AppResult<PageResult<RoleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let page = self
            .role_repo
            .find_by_page_filtered(
                &db,
                tenant_id,
                params.page.clone(),
                params.name.as_deref(),
                params.code.as_deref(),
                params.status.as_deref(),
            )
            .await?;
        let records = page.records.into_iter().map(RoleVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 查询当前操作者实际可以分配的角色候选项。
    pub async fn find_options(
        &self,
        actor: &ActorContext,
        query: Option<&str>,
        limit: u64,
    ) -> AppResult<OptionList> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| AppError::Config("角色选择器 limit 无法执行加一查询".into()))?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let mut roles = self
            .role_repo
            .find_options(&db, tenant_id, query, actor.is_super_admin, fetch_limit)
            .await?;
        let has_more = roles.len() > limit as usize;
        roles.truncate(limit as usize);
        Ok(OptionList {
            items: roles
                .into_iter()
                .map(|role| OptionItem {
                    value: role.id.to_string(),
                    label: role.name,
                    description: Some(role.code),
                    disabled: role.status != role::Model::STATUS_NORMAL,
                })
                .collect(),
            has_more,
        })
    }

    /// 以稳定主键游标分批读取角色导出数据。
    pub async fn find_for_export(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        upper_id: i64,
        maximum_records: usize,
    ) -> AppResult<Vec<RoleVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let filter = RoleFilter { name, code, status };
        let mut after_id = None;
        let mut records = Vec::new();
        loop {
            let batch = self
                .role_repo
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
            after_id = batch.last().map(|role| role.id);
            records.extend(batch.into_iter().map(RoleVo::from));
            if records.len() > maximum_records {
                return Err(AppError::Validation(format!(
                    "导出记录数超过 {maximum_records} 条上限"
                )));
            }
        }
        Ok(records)
    }

    /// 批量删除角色
    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的角色".into()));
        }

        let mut ids = ids.to_vec();
        normalize_ids(&mut ids);
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        // 每次创建、更新或删除角色时，先锁定租户行；随后按 ID 升序获取批量目标锁。
        let operation: AppResult<(u64, i32)> = async {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let mut roles = Vec::with_capacity(ids.len());
            for id in &ids {
                let role = self
                    .role_repo
                    .find_by_id_for_update(&transaction, tenant_id, *id)
                    .await?
                    .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
                roles.push(role);
            }

            let active_super_roles = roles
                .iter()
                .filter(|role| role.is_super == 1 && role.status == role::Model::STATUS_NORMAL)
                .count();
            if active_super_roles > 0 {
                let available = self
                    .role_repo
                    .count_available_super_roles_for_update(&transaction, tenant_id)
                    .await?;
                Self::ensure_super_role_remains(available, active_super_roles)?;
            }

            let affected = self
                .role_repo
                .delete_many(&transaction, tenant_id, &ids)
                .await?;
            if affected != ids.len() as u64 {
                return Err(AppError::NotFound("角色不存在".into()));
            }
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            Ok((affected, authorization_epoch))
        }
        .await;

        match operation {
            Ok((affected, authorization_epoch)) => {
                crate::commit_current_audit(transaction).await?;
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, authorization_epoch)
                    .await?;
                Ok(affected)
            }
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "批量删除角色事务回滚失败");
                }
                Err(error)
            }
        }
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<RoleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        match self.role_repo.find_by_id(&db, tenant_id, id).await? {
            Some(r) => {
                let mut vo = RoleVo::from(r);
                // 如果是自定义数据权限，查出关联的部门ID列表
                if vo.data_scope == "2" {
                    let dept_ids = self
                        .role_repo
                        .find_role_dept_ids(&db, tenant_id, id)
                        .await?;
                    vo.dept_ids = Some(dept_ids.iter().map(|d| d.to_string()).collect());
                }
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    pub async fn get_super_role(&self, actor: &ActorContext) -> AppResult<role::Model> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.role_repo
            .find_super_role(db, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("超级管理员角色不存在".into()))
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        name: &str,
        code: &str,
        sort: i32,
        data_scope: Option<String>,
    ) -> AppResult<RoleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let data_scope = data_scope.unwrap_or_else(|| "1".to_string());
        Self::validate_data_scope(&data_scope)?;
        let mut new_role = role::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            code: code.to_string(),
            is_super: 0,
            data_scope,
            status: "1".to_string(),
            sort,
            remark: None,
            del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_role.fill_on_insert(&FillContext::new())?;

        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let operation: AppResult<(role::Model, i32)> = async {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            if role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Code.eq(code))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .is_some()
            {
                return Err(AppError::Conflict("角色编码已存在".into()));
            }
            TenantRepository
                .ensure_role_quota_in_txn(&transaction, tenant_id)
                .await?;
            let saved = role::ActiveModel::from(new_role)
                .insert(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            Ok((saved, authorization_epoch))
        }
        .await;

        match operation {
            Ok((saved, authorization_epoch)) => {
                crate::commit_current_audit(transaction).await?;
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, authorization_epoch)
                    .await?;
                Ok(RoleVo::from(saved))
            }
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "创建角色事务回滚失败");
                }
                Err(error)
            }
        }
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        id: i64,
        name: &str,
        sort: i32,
        status: String,
        data_scope: Option<String>,
    ) -> AppResult<RoleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Self::validate_status(&status)?;
        if let Some(data_scope) = data_scope.as_deref() {
            Self::validate_data_scope(data_scope)?;
        }

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let operation: AppResult<(role::Model, i32)> = async {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let mut role = self
                .role_repo
                .find_by_id_for_update(&transaction, tenant_id, id)
                .await?
                .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;

            if role.is_super == 1
                && role.status == role::Model::STATUS_NORMAL
                && status != role::Model::STATUS_NORMAL
            {
                let available = self
                    .role_repo
                    .count_available_super_roles_for_update(&transaction, tenant_id)
                    .await?;
                Self::ensure_super_role_remains(available, 1)?;
            }

            role.name = name.to_string();
            role.sort = sort;
            role.status = status;
            if let Some(ds) = data_scope {
                role.data_scope = ds;
            }
            role.fill_on_update(&FillContext::new())?;

            let saved = role::ActiveModel::from(role)
                .reset_all()
                .update(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            Ok((saved, authorization_epoch))
        }
        .await;

        match operation {
            Ok((saved, authorization_epoch)) => {
                crate::commit_current_audit(transaction).await?;
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, authorization_epoch)
                    .await?;
                Ok(RoleVo::from(saved))
            }
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "更新角色事务回滚失败");
                }
                Err(error)
            }
        }
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        self.delete_many(actor, &[id]).await?;
        Ok(())
    }

    pub async fn assign_permissions(
        &self,
        actor: &ActorContext,
        role_id: i64,
        mut perm_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        normalize_ids(&mut perm_ids);
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.role_repo
            .find_by_id_for_update(&transaction, tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        let permissions = ryframe_db::entities::permission::Entity::find()
            .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
            .filter(ryframe_db::entities::permission::Column::Id.is_in(perm_ids.iter().copied()))
            .order_by_asc(ryframe_db::entities::permission::Column::Id)
            .lock(LockType::Update)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if let Some(perm_id) = first_missing_id(&perm_ids, &permissions, |permission| permission.id)
        {
            return Err(AppError::NotFound(format!("权限不存在: {perm_id}")));
        }
        let permission_codes = permissions
            .iter()
            .map(|permission| permission.code.clone())
            .collect::<Vec<_>>();
        self.product_service
            .ensure_permission_codes_enabled_in_txn(&transaction, tenant_id, &permission_codes)
            .await?;
        self.perm_repo
            .assign_perms(&transaction, tenant_id, role_id, &perm_ids)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(())
    }

    /// 返回分配给一个角色的全部已启用 API 权限码。
    pub async fn get_role_perm_codes(
        &self,
        actor: &ActorContext,
        role_id: i64,
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.role_repo
            .find_by_id(&db, tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        let mut codes: Vec<String> = self
            .perm_repo
            .find_role_perms(&db, tenant_id, &[role_id])
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect();
        codes.sort();
        codes.dedup();
        Ok(codes)
    }

    /// 原子替换一个角色的数据范围模式和自定义部门。
    pub async fn replace_data_scope(
        &self,
        actor: &ActorContext,
        role_id: i64,
        data_scope: &str,
        dept_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Self::validate_data_scope(data_scope)?;
        let unique_dept_ids = if data_scope == role::Model::DATA_SCOPE_CUSTOM {
            let mut unique_dept_ids = dept_ids;
            unique_dept_ids.sort_unstable();
            unique_dept_ids.dedup();
            if unique_dept_ids.is_empty() {
                return Err(AppError::Validation(
                    "自定义数据权限至少需要一个部门".into(),
                ));
            }
            unique_dept_ids
        } else {
            if !dept_ids.is_empty() {
                return Err(AppError::Validation("非自定义数据权限不能携带部门".into()));
            }
            Vec::new()
        };

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.role_repo
            .find_by_id_for_update(&transaction, tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        if data_scope == role::Model::DATA_SCOPE_CUSTOM {
            let existing_depts = ryframe_db::entities::dept::Entity::find()
                .filter(ryframe_db::entities::dept::Column::TenantId.eq(tenant_id))
                .filter(
                    ryframe_db::entities::dept::Column::DelFlag
                        .eq(ryframe_db::entities::dept::Model::DEL_FLAG_NORMAL),
                )
                .filter(
                    ryframe_db::entities::dept::Column::Id.is_in(unique_dept_ids.iter().copied()),
                )
                .order_by_asc(ryframe_db::entities::dept::Column::Id)
                .lock(LockType::Update)
                .all(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            if let Some(dept_id) =
                first_missing_id(&unique_dept_ids, &existing_depts, |dept| dept.id)
            {
                return Err(AppError::Validation(format!(
                    "自定义数据权限包含不存在或跨租户的部门: {dept_id}"
                )));
            }
        }
        self.role_repo
            .replace_data_scope(
                &transaction,
                tenant_id,
                role_id,
                data_scope,
                &unique_dept_ids,
            )
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(())
    }
}
use std::sync::Arc;

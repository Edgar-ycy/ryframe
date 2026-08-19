use ryframe_adapters::auto_fill::{AutoFill, FillContext};
use ryframe_db::{
    TenantConfigTransferRepository,
    entities::{dept, role_dept, user},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use super::{CreateDeptCommand, DeptService, DeptVo, UpdateDeptCommand};

impl DeptService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreateDeptCommand,
    ) -> AppResult<DeptVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let ancestors = match command.parent_id {
            None => "0".to_owned(),
            Some(parent_id) => {
                let parent = dept::Entity::find_by_id(parent_id)
                    .filter(dept::Column::TenantId.eq(tenant_id))
                    .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                    .lock(LockType::Update)
                    .one(&transaction)
                    .await
                    .map_err(|error| AppError::Database(error.to_string()))?
                    .ok_or_else(|| AppError::NotFound("父部门不存在".into()))?;
                format!("{},{}", parent.ancestors, parent_id)
            }
        };
        let mut new_dept = dept::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: command.name,
            parent_id: command.parent_id,
            ancestors,
            sort: command.sort,
            status: dept::Model::STATUS_NORMAL.to_owned(),
            remark: None,
            del_flag: dept::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_dept.fill_on_insert(&FillContext::new())?;
        let saved = self
            .dept_repo
            .insert_in_transaction(&transaction, tenant_id, new_dept)
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
        Ok(DeptVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdateDeptCommand,
    ) -> AppResult<DeptVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let id = command.id;
        if command.parent_id == Some(id) {
            return Err(AppError::Validation("部门不能将自己设为上级".into()));
        }
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut current = self
            .dept_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;
        let parent_changed = current.parent_id != command.parent_id;
        let old_ancestors = current.ancestors.clone();
        let mut descendants = Vec::new();
        if parent_changed {
            let old_prefix = format!("{old_ancestors},{id}");
            descendants = dept::Entity::find()
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .filter(
                    Condition::any()
                        .add(dept::Column::Ancestors.eq(&old_prefix))
                        .add(dept::Column::Ancestors.like(format!("{old_prefix},%"))),
                )
                .order_by_asc(dept::Column::Id)
                .lock(LockType::Update)
                .all(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            if command
                .parent_id
                .is_some_and(|parent| descendants.iter().any(|item| item.id == parent))
            {
                return Err(AppError::Validation(
                    "不能将部门移动到自己的后代节点".into(),
                ));
            }
            current.ancestors = match command.parent_id {
                None => "0".to_owned(),
                Some(parent_id) => {
                    let parent = dept::Entity::find_by_id(parent_id)
                        .filter(dept::Column::TenantId.eq(tenant_id))
                        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                        .lock(LockType::Update)
                        .one(&transaction)
                        .await
                        .map_err(|error| AppError::Database(error.to_string()))?
                        .ok_or_else(|| AppError::NotFound("父部门不存在".into()))?;
                    format!("{},{}", parent.ancestors, parent_id)
                }
            };
        }
        current.name = command.name;
        current.parent_id = command.parent_id;
        current.sort = command.sort;
        current.status = command.status;
        current.fill_on_update(&FillContext::new())?;
        let new_ancestors = current.ancestors.clone();
        let saved = self
            .dept_repo
            .update_in_transaction(&transaction, tenant_id, current)
            .await?;
        if parent_changed {
            let old_prefix = format!("{old_ancestors},{id}");
            let new_prefix = format!("{new_ancestors},{id}");
            for mut child in descendants {
                let suffix = child
                    .ancestors
                    .strip_prefix(&old_prefix)
                    .ok_or_else(|| AppError::Internal("部门祖级路径不一致，无法移动子树".into()))?;
                child.ancestors = format!("{new_prefix}{suffix}");
                child.fill_on_update(&FillContext::new())?;
                dept::ActiveModel::from(child)
                    .reset_all()
                    .update(&transaction)
                    .await
                    .map_err(|error| AppError::Database(error.to_string()))?;
            }
        }
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
        Ok(DeptVo::from(saved))
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
        self.dept_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;
        if dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .filter(dept::Column::ParentId.eq(id))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some()
        {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }
        let has_user_reference = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::DeptId.eq(id))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some();
        let has_role_reference = role_dept::Entity::find()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::DeptId.eq(id))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some();
        if has_user_reference || has_role_reference {
            return Err(AppError::Conflict(
                "部门仍被用户或角色数据权限引用，无法删除".into(),
            ));
        }
        self.dept_repo
            .delete_in_transaction(&transaction, tenant_id, id)
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

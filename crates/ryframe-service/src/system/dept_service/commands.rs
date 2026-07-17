use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::entities::dept;
use sea_orm::{ActiveModelTrait, TransactionTrait};

use super::{CreateDeptCommand, DeptService, DeptVo, UpdateDeptCommand};

impl DeptService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreateDeptCommand,
    ) -> AppResult<DeptVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let ancestors = self
            .dept_repo
            .build_ancestors(db, tenant_id, command.parent_id)
            .await?;

        let mut new_dept = dept::Model {
            id: snowflake::next_snowflake_id(),
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
        new_dept.fill_on_insert(&FillContext::new());
        let saved = self.dept_repo.insert(db, tenant_id, new_dept).await?;
        self.invalidate_dept_cache(tenant_id).await;
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
        let mut dept = self
            .dept_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        let parent_changed = dept.parent_id != command.parent_id;
        if command.parent_id == Some(id) {
            return Err(AppError::Validation("部门不能将自己设为上级".into()));
        }
        let old_ancestors = dept.ancestors.clone();
        let descendants = if parent_changed {
            let descendants = self.dept_repo.find_descendants(db, tenant_id, id).await?;
            if command
                .parent_id
                .is_some_and(|parent| descendants.iter().any(|item| item.id == parent))
            {
                return Err(AppError::Validation(
                    "不能将部门移动到自己的后代节点".into(),
                ));
            }
            dept.ancestors = self
                .dept_repo
                .build_ancestors(db, tenant_id, command.parent_id)
                .await?;
            descendants
        } else {
            Vec::new()
        };

        dept.name = command.name;
        dept.parent_id = command.parent_id;
        dept.sort = command.sort;
        dept.status = command.status;
        dept.fill_on_update(&FillContext::new());

        if !parent_changed {
            let saved = self.dept_repo.update(db, tenant_id, dept).await?;
            self.invalidate_dept_cache(tenant_id).await;
            return Ok(DeptVo::from(saved));
        }

        let new_ancestors = dept.ancestors.clone();
        let old_prefix = format!("{old_ancestors},{id}");
        let new_prefix = format!("{new_ancestors},{id}");
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let saved = dept::ActiveModel::from(dept)
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        for mut child in descendants {
            let suffix = child
                .ancestors
                .strip_prefix(&old_prefix)
                .ok_or_else(|| AppError::Internal("部门祖级路径不一致，无法移动子树".into()))?;
            child.ancestors = format!("{new_prefix}{suffix}");
            child.fill_on_update(&FillContext::new());
            dept::ActiveModel::from(child)
                .update(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.invalidate_dept_cache(tenant_id).await;
        Ok(DeptVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.dept_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        if self.dept_repo.has_children(db, tenant_id, id).await? {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }
        if self.dept_repo.is_referenced(db, tenant_id, id).await? {
            return Err(AppError::Conflict(
                "部门仍被用户或角色数据权限引用，无法删除".into(),
            ));
        }

        self.dept_repo.delete(db, tenant_id, id).await?;
        self.invalidate_dept_cache(tenant_id).await;
        Ok(())
    }
}

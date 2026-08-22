use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::ports::system::{DeptRecord, DeptWriteTransaction};

use super::{CreateDeptCommand, DeptService, DeptVo, UpdateDeptCommand};

const DEPT_STATUS_NORMAL: &str = "1";

impl DeptService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreateDeptCommand,
    ) -> AppResult<DeptVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let ancestors = self
            .parent_ancestors(transaction.as_ref(), tenant_id, command.parent_id)
            .await?;
        let saved = transaction
            .insert(
                tenant_id,
                DeptRecord {
                    id: crate::next_id()?,
                    name: command.name,
                    parent_id: command.parent_id,
                    ancestors,
                    sort: command.sort,
                    status: DEPT_STATUS_NORMAL.into(),
                    remark: None,
                    created_at: Default::default(),
                    updated_at: Default::default(),
                },
            )
            .await?;
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdateDeptCommand,
    ) -> AppResult<DeptVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let id = command.id;
        if command.parent_id == Some(id) {
            return Err(AppError::Validation("部门不能将自己设为上级".into()));
        }
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut current = transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;
        let parent_changed = current.parent_id != command.parent_id;
        let rewrite = if parent_changed {
            let old_ancestors = std::mem::take(&mut current.ancestors);
            let old_prefix = format!("{old_ancestors},{id}");
            let descendants = transaction
                .find_descendants_for_update(tenant_id, &old_prefix)
                .await?;
            if command
                .parent_id
                .is_some_and(|parent| descendants.iter().any(|item| item.id == parent))
            {
                return Err(AppError::Validation(
                    "不能将部门移动到自己的后代节点".into(),
                ));
            }
            current.ancestors = self
                .parent_ancestors(transaction.as_ref(), tenant_id, command.parent_id)
                .await?;
            let new_prefix = format!("{},{id}", current.ancestors);
            Some((old_prefix, new_prefix, descendants))
        } else {
            None
        };
        current.name = command.name;
        current.parent_id = command.parent_id;
        current.sort = command.sort;
        current.status = command.status;
        let saved = transaction.update(tenant_id, current).await?;

        if let Some((old_prefix, new_prefix, descendants)) = rewrite {
            for mut child in descendants {
                child.ancestors =
                    rewrite_descendant_ancestors(&child.ancestors, &old_prefix, &new_prefix)?;
                transaction.update(tenant_id, child).await?;
            }
        }
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;
        if transaction.has_child_for_update(tenant_id, id).await? {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }
        if transaction.has_reference_for_update(tenant_id, id).await? {
            return Err(AppError::Conflict(
                "部门仍被用户或角色数据权限引用，无法删除".into(),
            ));
        }
        transaction.delete(tenant_id, id).await?;
        self.commit_mutation(transaction, tenant_id).await
    }

    async fn parent_ancestors(
        &self,
        transaction: &dyn DeptWriteTransaction,
        tenant_id: &str,
        parent_id: Option<i64>,
    ) -> AppResult<String> {
        match parent_id {
            None => Ok("0".into()),
            Some(parent_id) => transaction
                .find_by_id_for_update(tenant_id, parent_id)
                .await?
                .map(|parent| format!("{},{parent_id}", parent.ancestors))
                .ok_or_else(|| AppError::NotFound("父部门不存在".into())),
        }
    }

    async fn commit_mutation(
        &self,
        transaction: Box<dyn DeptWriteTransaction>,
        tenant_id: &str,
    ) -> AppResult<()> {
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
    }
}

fn rewrite_descendant_ancestors(
    ancestors: &str,
    old_prefix: &str,
    new_prefix: &str,
) -> AppResult<String> {
    ancestors
        .strip_prefix(old_prefix)
        .map(|suffix| format!("{new_prefix}{suffix}"))
        .ok_or_else(|| AppError::Internal("部门祖级路径不一致，无法移动子树".into()))
}

#[cfg(test)]
mod tests {
    use super::rewrite_descendant_ancestors;

    #[test]
    fn descendant_path_rewrite_preserves_suffix_and_rejects_mismatch() {
        assert_eq!(
            rewrite_descendant_ancestors("0,10,20,30", "0,10", "0,40").unwrap(),
            "0,40,20,30"
        );
        assert!(rewrite_descendant_ancestors("0,11,20", "0,10", "0,40").is_err());
    }
}

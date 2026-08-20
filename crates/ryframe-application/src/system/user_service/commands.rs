use ryframe_auth::password;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use uuid::Uuid;

use super::{
    CreateUserParams, USER_STATUS_DISABLED, USER_STATUS_NORMAL, UpdateUserParams, UserService,
    UserVo,
};
use crate::{ManageableUserState, NewUserRecord, UpdateUserRecord, UserWriteTransaction};

impl UserService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let CreateUserParams {
            username,
            nickname,
            email,
            phone,
            dept_id,
            mut role_ids,
        } = params;
        normalize_ids(&mut role_ids);
        self.validate_assignments(actor, dept_id, Some(&role_ids))
            .await?;
        if self.writes.username_exists(tenant_id, username).await? {
            return Err(AppError::Conflict("用户名已存在".into()));
        }

        let activation_secret = format!("pending:{}", Uuid::new_v4());
        let transaction = self.writes.begin().await?;
        let result = async {
            transaction.lock_configuration(tenant_id).await?;
            self.validate_assignments_in_txn(transaction.as_ref(), actor, dept_id, Some(&role_ids))
                .await?;
            transaction.ensure_user_quota(tenant_id).await?;
            let saved = transaction
                .insert_user(NewUserRecord {
                    id: crate::next_id()?,
                    tenant_id: tenant_id.to_owned(),
                    username: username.to_owned(),
                    password_hash: password::hash(&activation_secret)?,
                    nickname: nickname.to_owned(),
                    email: email.to_owned(),
                    phone: phone.to_owned(),
                    dept_id,
                })
                .await?;
            if !role_ids.is_empty() {
                transaction
                    .replace_roles(tenant_id, saved.id, &role_ids)
                    .await?;
            }
            Ok(saved)
        }
        .await;
        match result {
            Ok(saved) => {
                transaction.commit().await?;
                Ok(UserVo::from(saved))
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        params: UpdateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let UpdateUserParams {
            id,
            nickname,
            email,
            phone,
            dept_id,
        } = params;
        self.validate_assignments(actor, dept_id, None).await?;
        let transaction = self.writes.begin().await?;
        let result = async {
            transaction.lock_configuration(tenant_id).await?;
            self.validate_assignments_in_txn(transaction.as_ref(), actor, dept_id, None)
                .await?;
            self.lock_manageable_user_in_txn(transaction.as_ref(), actor, id)
                .await?;
            let saved = transaction
                .update_user(
                    tenant_id,
                    UpdateUserRecord {
                        id,
                        nickname: nickname.to_owned(),
                        email: email.to_owned(),
                        phone: phone.to_owned(),
                        dept_id,
                    },
                )
                .await?;
            let versions = transaction
                .increment_authorization_versions(tenant_id, &[saved.id])
                .await?;
            Ok((saved, versions))
        }
        .await;
        match result {
            Ok((saved, versions)) => {
                transaction.commit().await?;
                self.authorization_cache
                    .sync_user_versions(tenant_id, &versions)
                    .await?;
                Ok(UserVo::from(saved))
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn update_status(
        &self,
        actor: &ActorContext,
        id: i64,
        status: String,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_manageable_status(&status)?;
        if id == actor.user_id && status != USER_STATUS_NORMAL {
            return Err(AppError::Authorization("禁止停用自己".into()));
        }
        let transaction = self.writes.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            self.lock_manageable_user_in_txn(transaction.as_ref(), actor, id)
                .await?;
            transaction.update_status(tenant_id, id, status).await?;
            transaction
                .increment_authorization_versions(tenant_id, &[id])
                .await
        }
        .await;
        self.commit_and_sync(transaction, tenant_id, result).await
    }

    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的用户".into()));
        }
        let mut ids = ids.to_vec();
        normalize_ids(&mut ids);
        if ids.contains(&actor.user_id) {
            return Err(AppError::Authorization("禁止删除自己".into()));
        }
        let transaction = self.writes.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            // 始终按升序获取用户锁，避免重叠批量操作按不同顺序请求相同用户时死锁。
            for id in &ids {
                self.lock_manageable_user_in_txn(transaction.as_ref(), actor, *id)
                    .await?;
            }
            let versions = transaction
                .increment_authorization_versions(tenant_id, &ids)
                .await?;
            let affected = transaction.delete_users(tenant_id, &ids).await?;
            if affected != ids.len() as u64 {
                return Err(AppError::NotFound("用户不存在".into()));
            }
            Ok((affected, versions))
        }
        .await;
        match result {
            Ok((affected, versions)) => {
                transaction.commit().await?;
                self.authorization_cache
                    .sync_user_versions(tenant_id, &versions)
                    .await?;
                Ok(affected)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        self.delete_many(actor, &[id]).await.map(|_| ())
    }

    pub(super) async fn lock_manageable_user_in_txn(
        &self,
        transaction: &dyn UserWriteTransaction,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ManageableUserState> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let state = transaction
            .lock_manageable_user(tenant_id, id, &actor.data_scope_context())
            .await?
            .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))?;
        if state.has_super_role {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }
        Ok(state)
    }

    async fn commit_and_sync(
        &self,
        transaction: Box<dyn UserWriteTransaction>,
        tenant_id: &str,
        result: AppResult<Vec<(i64, i32)>>,
    ) -> AppResult<()> {
        match result {
            Ok(versions) => {
                transaction.commit().await?;
                self.authorization_cache
                    .sync_user_versions(tenant_id, &versions)
                    .await
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}

pub(super) fn normalize_ids(ids: &mut Vec<i64>) {
    ids.sort_unstable();
    ids.dedup();
}

fn validate_manageable_status(status: &str) -> AppResult<()> {
    if matches!(status, USER_STATUS_NORMAL | USER_STATUS_DISABLED) {
        Ok(())
    } else {
        Err(AppError::Validation("无效的用户状态".into()))
    }
}

#[cfg(test)]
mod tests {
    use crate::USER_STATUS_PENDING_ACTIVATION;

    use super::*;

    #[test]
    fn user_ids_are_sorted_and_deduplicated_before_locking() {
        let mut ids = vec![9, 2, 9, 5];
        normalize_ids(&mut ids);
        assert_eq!(ids, vec![2, 5, 9]);
    }

    #[test]
    fn only_manageable_statuses_are_accepted() {
        assert!(validate_manageable_status(USER_STATUS_NORMAL).is_ok());
        assert!(validate_manageable_status(USER_STATUS_DISABLED).is_ok());
        assert!(validate_manageable_status(USER_STATUS_PENDING_ACTIVATION).is_err());
    }
}

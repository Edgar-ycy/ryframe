use ryframe_kernel::{ActorContext, AppError, AppResult};

use super::{UserService, commands::normalize_ids};
use crate::ports::users::{UserAssignmentState, UserWriteTransaction};

impl UserService {
    pub async fn replace_roles(
        &self,
        actor: &ActorContext,
        user_id: i64,
        mut role_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        normalize_ids(&mut role_ids);
        self.validate_assignments(actor, None, Some(&role_ids))
            .await?;

        let transaction = self.writes.begin().await?;
        let result = async {
            transaction.lock_configuration(tenant_id).await?;
            self.validate_assignments_in_txn(transaction.as_ref(), actor, None, Some(&role_ids))
                .await?;
            self.lock_manageable_user_in_txn(transaction.as_ref(), actor, user_id)
                .await?;
            transaction
                .replace_roles(tenant_id, user_id, &role_ids)
                .await?;
            transaction
                .increment_authorization_versions(tenant_id, &[user_id])
                .await
        }
        .await;
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

    pub(super) async fn validate_assignments(
        &self,
        actor: &ActorContext,
        dept_id: Option<i64>,
        role_ids: Option<&[i64]>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let state = self
            .writes
            .assignment_state(tenant_id, dept_id, role_ids.unwrap_or_default())
            .await?;
        validate_assignment_state(state, dept_id, role_ids, actor.is_super_admin)
    }

    pub(super) async fn validate_assignments_in_txn(
        &self,
        transaction: &dyn UserWriteTransaction,
        actor: &ActorContext,
        dept_id: Option<i64>,
        role_ids: Option<&[i64]>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let state = transaction
            .assignment_state(tenant_id, dept_id, role_ids.unwrap_or_default())
            .await?;
        validate_assignment_state(state, dept_id, role_ids, actor.is_super_admin)
    }
}

fn validate_assignment_state(
    state: UserAssignmentState,
    dept_id: Option<i64>,
    role_ids: Option<&[i64]>,
    actor_is_super: bool,
) -> AppResult<()> {
    if dept_id.is_some() && !state.department_exists {
        return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
    }
    let Some(role_ids) = role_ids else {
        return Ok(());
    };
    if state.roles.len() != role_ids.len() {
        return Err(AppError::Validation("角色不存在或不属于当前租户".into()));
    }
    if state.roles.iter().any(|role| !role.status_normal) {
        return Err(AppError::Validation("不能分配已停用的角色".into()));
    }
    if !actor_is_super && state.roles.iter().any(|role| role.is_super) {
        return Err(AppError::Authorization("无权限分配超级管理员角色".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::ports::users::UserAssignmentRole;

    use super::*;

    fn state(roles: Vec<UserAssignmentRole>) -> UserAssignmentState {
        UserAssignmentState {
            department_exists: true,
            roles,
        }
    }

    #[test]
    fn assignment_validation_fails_closed() {
        let normal = UserAssignmentRole {
            status_normal: true,
            is_super: false,
        };
        assert!(validate_assignment_state(state(vec![normal]), None, Some(&[1]), false).is_ok());
        assert!(validate_assignment_state(state(Vec::new()), None, Some(&[1]), false).is_err());
        assert!(
            validate_assignment_state(
                state(vec![UserAssignmentRole {
                    status_normal: false,
                    is_super: false,
                }]),
                None,
                Some(&[1]),
                false,
            )
            .is_err()
        );
        assert!(
            validate_assignment_state(
                state(vec![UserAssignmentRole {
                    status_normal: true,
                    is_super: true,
                }]),
                None,
                Some(&[1]),
                false,
            )
            .is_err()
        );
    }
}

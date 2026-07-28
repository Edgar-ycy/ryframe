use std::collections::HashMap;

use ryframe_core::{Repository, repository::PageResult};
use ryframe_db::UserFilter;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::DatabaseConnection;

use super::{RoleBriefVo, UserDetailVo, UserListParams, UserService, UserVo};

impl UserService {
    /// 以稳定的主键游标分批读取可导出的用户，避免大页码查询在并发写入时产生重复或遗漏。
    pub async fn find_for_export(
        &self,
        actor: &ActorContext,
        params: &UserListParams,
        maximum_records: usize,
    ) -> AppResult<Vec<UserVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        let filter = UserFilter {
            username: params.username.as_deref(),
            phone: params.phone.as_deref(),
            status: params.status.as_deref(),
            dept_id: params.dept_id,
        };
        let mut after_id = None;
        let mut records = Vec::new();

        loop {
            let batch = self
                .user_repo
                .find_for_export_after_id(&db, tenant_id, &filter, &scope, after_id, BATCH_SIZE)
                .await?;
            if batch.is_empty() {
                break;
            }
            after_id = batch.last().map(|user| user.id);
            records.extend(batch.into_iter().map(UserVo::from));
            if records.len() > maximum_records {
                return Err(AppError::Validation(format!(
                    "导出记录数超过 {maximum_records} 条上限"
                )));
            }
        }

        self.fill_dept_names(&db, tenant_id, &mut records).await?;
        Ok(records)
    }

    /// 在异步任务真正执行前按数据库当前状态重新校验申请人的账号和权限。
    ///
    /// 数据范围继续使用任务创建时保存的快照，防止等待期间扩大可导出范围；权限则必须
    /// 使用当前角色和授权重新计算，确保撤权或停用会立即阻止任务继续执行。
    pub async fn ensure_current_permission(
        &self,
        actor: &ActorContext,
        permission_code: &str,
    ) -> AppResult<()> {
        if permission_code.trim().is_empty() {
            return Err(AppError::Validation("权限代码不能为空".into()));
        }
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        let user = self
            .user_repo
            .find_by_id(&db, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::Authorization("导出申请人已不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authorization("导出申请人的账号已停用".into()));
        }
        let roles = self
            .role_repo
            .find_user_roles(&db, tenant_id, actor.user_id)
            .await?;
        if roles.iter().any(|role| role.is_super == 1) {
            return Ok(());
        }
        let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
        let permissions = self
            .perm_repo
            .find_role_perms(&db, tenant_id, &role_ids)
            .await?;
        let permission_codes = permissions
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        if ryframe_auth::rbac::has_permission(&permission_codes, permission_code) {
            Ok(())
        } else {
            Err(AppError::Authorization("导出申请人的权限已被撤销".into()))
        }
    }

    async fn fill_dept_names(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        records: &mut [UserVo],
    ) -> AppResult<()> {
        let mut dept_ids = records
            .iter()
            .filter_map(|user| user.dept_id.as_deref())
            .filter_map(|value| value.parse::<i64>().ok())
            .collect::<Vec<_>>();
        dept_ids.sort_unstable();
        dept_ids.dedup();

        let dept_names = self
            .dept_repo
            .find_filtered_by_ids(db, tenant_id, None, None, &dept_ids)
            .await?
            .into_iter()
            .map(|dept| (dept.id, dept.name))
            .collect::<HashMap<_, _>>();

        for user in records {
            user.dept_name = user
                .dept_id
                .as_deref()
                .and_then(|value| value.parse::<i64>().ok())
                .and_then(|dept_id| dept_names.get(&dept_id).cloned());
        }

        Ok(())
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: UserListParams,
    ) -> AppResult<PageResult<UserVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        let filter = UserFilter {
            username: params.username.as_deref(),
            phone: params.phone.as_deref(),
            status: params.status.as_deref(),
            dept_id: params.dept_id,
        };
        let page = self
            .user_repo
            .find_by_page_filtered_with_data_scope(&db, tenant_id, &params.page, &filter, &scope)
            .await?;
        let mut records = page
            .records
            .into_iter()
            .map(UserVo::from)
            .collect::<Vec<_>>();
        self.fill_dept_names(&db, tenant_id, &mut records).await?;
        Ok(PageResult::new(records, page.total, &params.page))
    }

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<Option<UserDetailVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        let Some(user) = self
            .user_repo
            .find_by_id_with_data_scope(&db, tenant_id, id, &scope)
            .await?
        else {
            return Ok(None);
        };

        let mut user = UserVo::from(user);
        self.fill_dept_names(&db, tenant_id, std::slice::from_mut(&mut user))
            .await?;
        let roles = self
            .role_repo
            .find_user_roles(&db, tenant_id, id)
            .await?
            .into_iter()
            .map(RoleBriefVo::from)
            .collect();
        Ok(Some(UserDetailVo { user, roles }))
    }

    pub async fn ensure_user_accessible(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        self.user_repo
            .find_by_id_with_data_scope(self.db.write(), tenant_id, id, &scope)
            .await?
            .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))
            .map(|_| ())
    }

    pub async fn is_super_admin_user(&self, actor: &ActorContext, id: i64) -> AppResult<bool> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_user_accessible(actor, id).await?;
        let roles = self
            .role_repo
            .find_user_roles_all_status(self.db.write(), tenant_id, id)
            .await?;
        Ok(roles.iter().any(|role| role.is_super == 1))
    }

    pub(super) async fn ensure_not_super_admin_user(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<()> {
        if self.is_super_admin_user(actor, id).await? {
            Err(AppError::Authorization("禁止操作超级管理员".into()))
        } else {
            Ok(())
        }
    }
}

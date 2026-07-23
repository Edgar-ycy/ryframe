use std::collections::HashMap;

use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::repository::PageResult;
use ryframe_db::UserFilter;
use sea_orm::DatabaseConnection;

use super::{RoleBriefVo, UserDetailVo, UserListParams, UserService, UserVo};

impl UserService {
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
            .find_by_page_filtered_with_data_scope(db, tenant_id, &params.page, &filter, &scope)
            .await?;
        let mut records = page
            .records
            .into_iter()
            .map(UserVo::from)
            .collect::<Vec<_>>();
        self.fill_dept_names(db, tenant_id, &mut records).await?;
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
            .find_by_id_with_data_scope(db, tenant_id, id, &scope)
            .await?
        else {
            return Ok(None);
        };

        let mut user = UserVo::from(user);
        self.fill_dept_names(db, tenant_id, std::slice::from_mut(&mut user))
            .await?;
        let roles = self
            .role_repo
            .find_user_roles(db, tenant_id, id)
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

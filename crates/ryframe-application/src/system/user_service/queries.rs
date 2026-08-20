use ryframe_kernel::{ActorContext, AppError, AppResult, ExportCursorWindow, PageResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{OptionItem, OptionList};
use super::{RoleBriefVo, UserDetailVo, UserListParams, UserService, UserVo};
use crate::{IdentityRoleRecord, IdentityTenantRecord, IdentityUserRecord, UserQueryFilter};

/// 从主库重新计算的当前授权结果，供长时间后台任务和诊断接口共用。
pub(crate) struct CurrentAuthorization {
    pub actor: ActorContext,
    pub tenant: IdentityTenantRecord,
    pub user: IdentityUserRecord,
    pub roles: Vec<IdentityRoleRecord>,
    pub permission_codes: Vec<String>,
    pub fingerprint: String,
}

impl UserService {
    /// 按调用方提供的稳定主键窗口读取一批可导出用户。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        filter: UserQueryFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<UserVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let records = self
            .queries
            .export_batch(tenant_id, filter, &scope, window)
            .await?
            .into_iter()
            .map(UserVo::from)
            .collect::<Vec<_>>();
        Ok(records)
    }

    /// 从主库重新计算导出申请人的当前账号、权限和数据范围。
    ///
    /// 返回的指纹覆盖租户授权纪元、用户授权版本、角色、权限和最终数据范围；任一项变化
    /// 都会产生不同指纹，使既有导出结果在下载时失效。
    pub(crate) async fn resolve_current_export_authorization(
        &self,
        tenant_id: &str,
        user_id: i64,
        permission_code: &str,
    ) -> AppResult<(ActorContext, String)> {
        let authorization = self
            .resolve_current_authorization(tenant_id, user_id, permission_code)
            .await?;
        Ok((authorization.actor, authorization.fingerprint))
    }

    /// 从主库重新计算指定用户的账号、角色、权限和最终数据范围。
    pub(crate) async fn resolve_current_authorization(
        &self,
        tenant_id: &str,
        user_id: i64,
        permission_code: &str,
    ) -> AppResult<CurrentAuthorization> {
        if permission_code.trim().is_empty() {
            return Err(AppError::Validation("权限代码不能为空".into()));
        }
        let authorization = self
            .calculate_current_authorization(tenant_id, user_id)
            .await?;
        if !authorization.tenant.is_available(chrono::Utc::now()) {
            return Err(AppError::Authorization(
                "操作申请人的租户已停用或到期".into(),
            ));
        }
        if !authorization.user.is_enabled() {
            return Err(AppError::Authorization("操作申请人的账号已停用".into()));
        }
        if !authorization.actor.is_super_admin
            && !ryframe_auth::rbac::has_permission(&authorization.permission_codes, permission_code)
        {
            return Err(AppError::Authorization("操作申请人的权限已被撤销".into()));
        }

        Ok(authorization)
    }

    /// 从主库计算授权，不因目标账号或租户停用而提前失败，供只读诊断展示使用。
    pub(crate) async fn calculate_current_authorization(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<CurrentAuthorization> {
        let tenant = self
            .authorization_resolver
            .tenant(tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let user = self
            .authorization_resolver
            .user_by_id(tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let resolved = self
            .authorization_resolver
            .resolve(tenant_id, &user)
            .await?;
        let is_super_admin = resolved.roles.iter().any(|role| role.is_super);

        let actor = ActorContext {
            user_id,
            tenant_id: tenant_id.to_owned(),
            username: user.username.clone(),
            dept_id: user.dept_id,
            dept_path: resolved.data_scope.ancestors.clone(),
            data_scope: resolved.data_scope.scope.clone(),
            custom_dept_ids: resolved.data_scope.custom_dept_ids.clone(),
            include_self: resolved.data_scope.include_self,
            is_super_admin,
        };
        let fingerprint = calculate_export_authorization_fingerprint(
            tenant.authorization_epoch,
            user.authorization_version,
            &actor,
            &resolved.roles,
            &resolved.permission_codes,
        )?;
        Ok(CurrentAuthorization {
            actor,
            tenant,
            user,
            roles: resolved.roles,
            permission_codes: resolved.permission_codes,
            fingerprint,
        })
    }

    /// 按数据库当前状态重新校验申请人的账号和权限。
    pub async fn ensure_current_permission(
        &self,
        actor: &ActorContext,
        permission_code: &str,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.resolve_current_export_authorization(tenant_id, actor.user_id, permission_code)
            .await
            .map(|_| ())
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: UserListParams,
    ) -> AppResult<PageResult<UserVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let filter = UserQueryFilter {
            username: params.username.as_deref(),
            phone: params.phone.as_deref(),
            status: params.status.as_deref(),
            dept_id: params.dept_id,
        };
        let page = self
            .queries
            .page(tenant_id, params.page, filter, &scope)
            .await?;
        let records = page
            .records
            .into_iter()
            .map(UserVo::from)
            .collect::<Vec<_>>();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 查询当前操作者数据范围内的用户候选项。
    pub async fn find_options(
        &self,
        actor: &ActorContext,
        query: Option<&str>,
        limit: u64,
    ) -> AppResult<OptionList> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| AppError::Config("用户选择器 limit 无法执行加一查询".into()))?;
        let scope = actor.data_scope_context();
        let mut users = self
            .queries
            .options(tenant_id, query, &scope, fetch_limit)
            .await?;
        let has_more = users.len() > limit as usize;
        users.truncate(limit as usize);
        Ok(OptionList {
            items: users
                .into_iter()
                .map(|user| {
                    let disabled = !user.is_enabled();
                    let description =
                        (user.nickname != user.username).then(|| user.username.clone());
                    OptionItem {
                        value: user.id.to_string(),
                        label: if user.nickname.is_empty() {
                            user.username
                        } else {
                            user.nickname
                        },
                        description,
                        disabled,
                    }
                })
                .collect(),
            has_more,
        })
    }

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<Option<UserDetailVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let Some(detail) = self.queries.detail(tenant_id, id, &scope).await? else {
            return Ok(None);
        };
        Ok(Some(UserDetailVo {
            user: UserVo::from(detail.user),
            roles: detail.roles.into_iter().map(RoleBriefVo::from).collect(),
        }))
    }

    pub async fn ensure_user_accessible(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        self.queries
            .is_accessible(tenant_id, id, &scope)
            .await?
            .then_some(())
            .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))
    }

    pub async fn is_super_admin_user(&self, actor: &ActorContext, id: i64) -> AppResult<bool> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_user_accessible(actor, id).await?;
        self.queries.is_super_admin(tenant_id, id).await
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

#[derive(Serialize)]
struct ExportRoleFingerprint<'a> {
    id: i64,
    code: &'a str,
    data_scope: &'a str,
    is_super: bool,
}

#[derive(Serialize)]
struct ExportAuthorizationFingerprint<'a> {
    tenant_authorization_epoch: i32,
    user_authorization_version: i32,
    actor: &'a ActorContext,
    roles: Vec<ExportRoleFingerprint<'a>>,
    permissions: Vec<&'a str>,
}

fn calculate_export_authorization_fingerprint(
    tenant_authorization_epoch: i32,
    user_authorization_version: i32,
    actor: &ActorContext,
    roles: &[IdentityRoleRecord],
    permission_codes: &[String],
) -> AppResult<String> {
    let mut roles = roles
        .iter()
        .map(|role| ExportRoleFingerprint {
            id: role.id,
            code: &role.code,
            data_scope: &role.data_scope,
            is_super: role.is_super,
        })
        .collect::<Vec<_>>();
    roles.sort_unstable_by_key(|role| role.id);
    let mut permissions = permission_codes
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    permissions.sort_unstable();
    permissions.dedup();
    let payload = ExportAuthorizationFingerprint {
        tenant_authorization_epoch,
        user_authorization_version,
        actor,
        roles,
        permissions,
    };
    let encoded = serde_json::to_vec(&payload)
        .map_err(|error| AppError::Internal(format!("导出授权指纹编码失败: {error}")))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

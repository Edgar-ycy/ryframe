use std::collections::HashMap;

use ryframe_core::{Repository, repository::PageResult};
use ryframe_db::{ReadConsistency, TenantRepository, UserFilter, entities::role};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope, DataScopeContext};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{OptionItem, OptionList};
use super::{RoleBriefVo, UserDetailVo, UserListParams, UserService, UserVo};

impl UserService {
    /// 以稳定的主键游标分批读取可导出的用户，避免大页码查询在并发写入时产生重复或遗漏。
    pub async fn find_for_export(
        &self,
        actor: &ActorContext,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
        maximum_records: usize,
    ) -> AppResult<Vec<UserVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let filter = UserFilter {
            username,
            phone,
            status,
            dept_id,
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
            after_id = validate_export_cursor_batch(
                &batch,
                after_id,
                records.len(),
                maximum_records,
                |user| user.id,
            )?;
            records.extend(batch.into_iter().map(UserVo::from));
        }

        self.fill_dept_names(&db, tenant_id, &mut records).await?;
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
        if permission_code.trim().is_empty() {
            return Err(AppError::Validation("权限代码不能为空".into()));
        }
        let db = self.db.write();
        let tenant = TenantRepository.ensure_available(db, tenant_id).await?;
        let user = self
            .user_repo
            .find_by_id(db, tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authorization("导出申请人已不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authorization("导出申请人的账号已停用".into()));
        }

        let mut roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user_id)
            .await?;
        roles.sort_unstable_by_key(|role| role.id);
        let is_super_admin = roles.iter().any(|role| role.is_super == 1);
        let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
        let mut permission_codes = self
            .perm_repo
            .find_role_perms(db, tenant_id, &role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        permission_codes.sort_unstable();
        permission_codes.dedup();
        if !is_super_admin
            && !ryframe_auth::rbac::has_permission(&permission_codes, permission_code)
        {
            return Err(AppError::Authorization("导出申请人的权限已被撤销".into()));
        }

        let data_scope = self
            .resolve_current_export_data_scope(db, tenant_id, user_id, user.dept_id, &roles)
            .await?;
        let actor = ActorContext {
            user_id,
            tenant_id: tenant_id.to_owned(),
            username: user.username,
            dept_id: user.dept_id,
            dept_path: data_scope.ancestors.clone(),
            data_scope: data_scope.scope,
            custom_dept_ids: data_scope.custom_dept_ids,
            include_self: data_scope.include_self,
            is_super_admin,
        };
        let fingerprint = calculate_export_authorization_fingerprint(
            tenant.authorization_epoch,
            user.authorization_version,
            &actor,
            &roles,
            &permission_codes,
        )?;
        Ok((actor, fingerprint))
    }

    async fn resolve_current_export_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        dept_id: Option<i64>,
        roles: &[role::Model],
    ) -> AppResult<DataScopeContext> {
        if roles.iter().any(|role| role.is_super == 1) {
            return Ok(DataScopeContext::super_admin(user_id));
        }
        let ancestors = match dept_id {
            Some(dept_id) => Some(
                self.dept_repo
                    .find_by_id(db, tenant_id, dept_id)
                    .await?
                    .ok_or_else(|| AppError::Authorization("导出申请人的部门已不存在".into()))?
                    .ancestors,
            ),
            None => None,
        };
        let custom_role_ids = roles
            .iter()
            .filter(|role| role.data_scope == role::Model::DATA_SCOPE_CUSTOM)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let custom_dept_ids = self
            .role_repo
            .find_roles_dept_ids(db, tenant_id, &custom_role_ids)
            .await?;
        let mut scopes = Vec::with_capacity(roles.len());
        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match dept_id {
                    Some(dept_id) => {
                        self.dept_repo
                            .find_child_dept_ids(db, tenant_id, dept_id)
                            .await?
                    }
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
            });
        }
        if scopes.is_empty() {
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id,
                dept_id,
                ancestors,
                custom_dept_ids: Vec::new(),
                include_self: true,
            });
        }
        Ok(DataScopeContext::merge(scopes))
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
        let db = self.db.select_read(ReadConsistency::Strong).connection;
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
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let mut users = self
            .user_repo
            .find_options_with_data_scope(&db, tenant_id, query, &scope, fetch_limit)
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
        let db = self.db.select_read(ReadConsistency::Strong).connection;
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

#[derive(Serialize)]
struct ExportRoleFingerprint<'a> {
    id: i64,
    code: &'a str,
    data_scope: &'a str,
    is_super: i8,
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
    roles: &[role::Model],
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

fn validate_export_cursor_batch<T>(
    batch: &[T],
    previous_id: Option<i64>,
    accumulated: usize,
    maximum_records: usize,
    id: impl Fn(&T) -> i64,
) -> AppResult<Option<i64>> {
    if batch.is_empty() {
        return Ok(None);
    }
    if accumulated.saturating_add(batch.len()) > maximum_records {
        return Err(AppError::Validation(format!(
            "导出记录数超过 {maximum_records} 条上限"
        )));
    }
    let mut cursor = previous_id;
    for item in batch {
        let current = id(item);
        if cursor.is_some_and(|previous| current <= previous) {
            return Err(AppError::Internal("导出主键游标未严格递增".into()));
        }
        cursor = Some(current);
    }
    Ok(cursor)
}

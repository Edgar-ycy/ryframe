use std::sync::Arc;

use chrono::Utc;
use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use serde::Serialize;

use crate::{
    AuthorizationCache,
    ports::system::{RoleFilter, RoleReadPort, RoleRecord, RoleWritePort},
};

use super::{OptionItem, OptionList};

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

impl From<RoleRecord> for RoleVo {
    fn from(role: RoleRecord) -> Self {
        Self {
            id: role.id.to_string(),
            name: role.name,
            code: role.code,
            is_super: role.is_super,
            data_scope: role.data_scope,
            status: role.status,
            sort: role.sort,
            remark: role.remark,
            created_at: role.created_at,
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

/// 角色选项的明确分配用途。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoleOptionPurpose {
    UserAssignment,
    ServiceAccountAssignment,
}

impl RoleOptionPurpose {
    pub const fn includes_super_role(self, actor_is_super_admin: bool) -> bool {
        matches!(self, Self::UserAssignment) && actor_is_super_admin
    }
}

pub struct RoleService {
    read: Arc<dyn RoleReadPort>,
    write: Arc<dyn RoleWritePort>,
    authorization_cache: AuthorizationCache,
}

impl RoleService {
    pub fn new(
        authorization_cache: AuthorizationCache,
        read: Arc<dyn RoleReadPort>,
        write: Arc<dyn RoleWritePort>,
    ) -> Self {
        Self {
            read,
            write,
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
        if matches!(status, "1" | "0") {
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

    pub async fn get_role_model(&self, actor: &ActorContext, id: i64) -> AppResult<RoleRecord> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.read
            .find_by_id(tenant_id, id)
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
        let page = self
            .read
            .find_by_page(
                tenant_id,
                params.page,
                RoleFilter {
                    name: params.name.as_deref(),
                    code: params.code.as_deref(),
                    status: params.status.as_deref(),
                },
            )
            .await?;
        let records = page.records.into_iter().map(RoleVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 查询当前操作者实际可以分配的角色候选项。
    pub async fn find_options(
        &self,
        actor: &ActorContext,
        purpose: RoleOptionPurpose,
        query: Option<&str>,
        limit: u64,
    ) -> AppResult<OptionList> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| AppError::Config("角色选择器 limit 无法执行加一查询".into()))?;
        let mut roles = self
            .read
            .find_options(
                tenant_id,
                query,
                purpose.includes_super_role(actor.is_super_admin),
                fetch_limit,
            )
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
                    disabled: role.status != "1",
                })
                .collect(),
            has_more,
        })
    }

    /// 按稳定主键窗口读取一批角色导出数据。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<RoleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let filter = RoleFilter { name, code, status };
        Ok(self
            .read
            .find_export_batch(tenant_id, filter, window)
            .await?
            .into_iter()
            .map(RoleVo::from)
            .collect())
    }

    /// 批量删除角色
    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的角色".into()));
        }

        let mut ids = ids.to_vec();
        normalize_ids(&mut ids);
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut roles = Vec::with_capacity(ids.len());
        for id in &ids {
            roles.push(
                transaction
                    .find_by_id_for_update(tenant_id, *id)
                    .await?
                    .ok_or_else(|| AppError::NotFound("角色不存在".into()))?,
            );
        }
        let active_super_roles = roles
            .iter()
            .filter(|role| role.is_super == 1 && role.status == "1")
            .count();
        if active_super_roles > 0 {
            let available = transaction.count_available_super_roles(tenant_id).await?;
            Self::ensure_super_role_remains(available, active_super_roles)?;
        }
        let affected = transaction.delete_many(tenant_id, &ids).await?;
        if affected != ids.len() as u64 {
            return Err(AppError::NotFound("角色不存在".into()));
        }
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(affected)
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<RoleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        match self.read.find_by_id(tenant_id, id).await? {
            Some(r) => {
                let mut vo = RoleVo::from(r);
                // 如果是自定义数据权限，查出关联的部门ID列表
                if vo.data_scope == "2" {
                    let dept_ids = self.read.find_role_dept_ids(tenant_id, id).await?;
                    vo.dept_ids = Some(dept_ids.iter().map(|d| d.to_string()).collect());
                }
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    pub async fn get_super_role(&self, actor: &ActorContext) -> AppResult<RoleRecord> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.read
            .find_super_role(tenant_id)
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
        let data_scope = data_scope.unwrap_or_else(|| "1".to_string());
        Self::validate_data_scope(&data_scope)?;
        let now = Utc::now();
        let record = RoleRecord {
            id: crate::next_id()?,
            name: name.to_owned(),
            code: code.to_owned(),
            is_super: 0,
            data_scope,
            status: "1".into(),
            sort,
            remark: None,
            created_at: now,
            updated_at: now,
        };
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        if transaction
            .find_by_code_for_update(tenant_id, code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("角色编码已存在".into()));
        }
        transaction.ensure_role_quota(tenant_id).await?;
        let saved = transaction.insert(tenant_id, record).await?;
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(RoleVo::from(saved))
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

        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut role = transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        if role.is_super == 1 && role.status == "1" && status != "1" {
            let available = transaction.count_available_super_roles(tenant_id).await?;
            Self::ensure_super_role_remains(available, 1)?;
        }
        role.name = name.to_owned();
        role.sort = sort;
        role.status = status;
        if let Some(data_scope) = data_scope {
            role.data_scope = data_scope;
        }
        role.updated_at = Utc::now();
        let saved = transaction.update(tenant_id, role).await?;
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(RoleVo::from(saved))
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
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        let permissions = transaction
            .find_permissions_for_update(tenant_id, &perm_ids)
            .await?;
        if let Some(perm_id) = first_missing_id(&perm_ids, &permissions, |permission| permission.id)
        {
            return Err(AppError::NotFound(format!("权限不存在: {perm_id}")));
        }
        let permission_codes = permissions
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        transaction
            .ensure_permission_codes_enabled(tenant_id, &permission_codes)
            .await?;
        transaction
            .assign_permissions(tenant_id, role_id, &perm_ids)
            .await?;
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
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
        self.read
            .find_permission_codes(tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))
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
        let unique_dept_ids = if data_scope == "2" {
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

        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        if data_scope == "2" {
            let existing_depts = transaction
                .find_departments_for_update(tenant_id, &unique_dept_ids)
                .await?;
            if let Some(dept_id) = first_missing_id(&unique_dept_ids, &existing_depts, |id| *id) {
                return Err(AppError::Validation(format!(
                    "自定义数据权限包含不存在或跨租户的部门: {dept_id}"
                )));
            }
        }
        transaction
            .replace_data_scope(tenant_id, role_id, data_scope, &unique_dept_ids)
            .await?;
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(())
    }
}

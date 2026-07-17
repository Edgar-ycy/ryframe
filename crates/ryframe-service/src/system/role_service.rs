use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    cache::clear_user_permission_cache,
    repository::{PageQuery, PageResult},
};
use ryframe_db::DatabaseCluster;
use ryframe_db::{DeptRepository, PermissionRepository, RoleRepository, entities::role};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
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

impl From<role::Model> for RoleVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id.to_string(),
            name: r.name,
            code: r.code,
            is_super: r.is_super,
            data_scope: r.data_scope,
            status: r.status,
            sort: r.sort,
            remark: r.remark,
            created_at: r.created_at,
            dept_ids: None,
        }
    }
}

#[derive(Debug)]
pub struct RoleListParams {
    pub page: PageQuery,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

pub struct RoleService {
    db: DatabaseCluster,
    role_repo: LoggedRepo<RoleRepository>,
    perm_repo: LoggedRepo<PermissionRepository>,
    dept_repo: LoggedRepo<DeptRepository>,
    redis: Option<RedisClient>,
}

impl RoleService {
    pub fn new(db: DatabaseCluster, redis: Option<RedisClient>) -> Self {
        Self {
            db,
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            dept_repo: LoggedRepo::new(DeptRepository),
            redis,
        }
    }

    fn validate_data_scope(data_scope: &str) -> AppResult<()> {
        if matches!(data_scope, "1" | "2" | "3" | "4" | "5") {
            Ok(())
        } else {
            Err(AppError::Validation("无效的数据范围值".into()))
        }
    }

    async fn invalidate_role_users(&self, tenant_id: &str, role_id: i64) -> AppResult<()> {
        let db = self.db.write();
        let user_ids = self
            .role_repo
            .find_user_ids_by_role_ids(db, tenant_id, &[role_id])
            .await?;
        if let Some(redis) = &self.redis {
            for user_id in user_ids {
                if let Err(error) = clear_user_permission_cache(redis, tenant_id, user_id).await {
                    tracing::warn!(role_id, user_id, %error, "failed to clear role user permission cache");
                }
            }
        }
        Ok(())
    }

    pub async fn get_role_model(&self, actor: &ActorContext, id: i64) -> AppResult<role::Model> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.role_repo
            .find_by_id(db, tenant_id, id)
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
        let db = self.db.read();
        let page = self
            .role_repo
            .find_by_page_filtered(
                db,
                tenant_id,
                params.page.clone(),
                params.name.as_deref(),
                params.code.as_deref(),
                params.status.as_deref(),
            )
            .await?;
        let records = page.records.into_iter().map(RoleVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 批量删除角色
    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的角色".into()));
        }
        for id in ids {
            self.get_role_model(actor, *id).await?;
        }
        let affected = self.role_repo.delete_many(db, tenant_id, ids).await?;
        for role_id in ids {
            self.invalidate_role_users(tenant_id, *role_id).await?;
        }
        Ok(affected)
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<RoleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        match self.role_repo.find_by_id(db, tenant_id, id).await? {
            Some(r) => {
                let mut vo = RoleVo::from(r);
                // 如果是自定义数据权限，查出关联的部门ID列表
                if vo.data_scope == "2" {
                    let dept_ids = self.role_repo.find_role_dept_ids(db, tenant_id, id).await?;
                    vo.dept_ids = Some(dept_ids.iter().map(|d| d.to_string()).collect());
                }
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    pub async fn get_super_role(&self, actor: &ActorContext) -> AppResult<role::Model> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.role_repo
            .find_super_role(db, tenant_id)
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
        let db = self.db.write();
        ryframe_db::TenantRepository
            .ensure_role_quota(db, tenant_id)
            .await?;
        if self
            .role_repo
            .find_by_code(db, tenant_id, code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("角色编码已存在".into()));
        }

        let data_scope = data_scope.unwrap_or_else(|| "1".to_string());
        Self::validate_data_scope(&data_scope)?;
        let mut new_role = role::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            code: code.to_string(),
            is_super: 0,
            data_scope,
            status: "1".to_string(),
            sort,
            remark: None,
            del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_role.fill_on_insert(&FillContext::new());

        let saved = self.role_repo.insert(db, tenant_id, new_role).await?;
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
        let db = self.db.write();
        let mut role = self.get_role_model(actor, id).await?;

        role.name = name.to_string();
        role.sort = sort;
        role.status = status;
        if let Some(ds) = data_scope {
            Self::validate_data_scope(&ds)?;
            role.data_scope = ds;
        }
        role.fill_on_update(&FillContext::new());

        let saved = self.role_repo.update(db, tenant_id, role).await?;
        self.invalidate_role_users(tenant_id, id).await?;
        Ok(RoleVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.get_role_model(actor, id).await?;
        self.role_repo.delete(db, tenant_id, id).await?;
        self.invalidate_role_users(tenant_id, id).await
    }

    pub async fn assign_permissions(
        &self,
        actor: &ActorContext,
        role_id: i64,
        perm_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.get_role_model(actor, role_id).await?;
        for perm_id in &perm_ids {
            self.perm_repo
                .find_by_id(db, tenant_id, *perm_id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("权限不存在: {}", perm_id)))?;
        }
        self.perm_repo
            .assign_perms(db, tenant_id, role_id, &perm_ids)
            .await?;
        self.invalidate_role_users(tenant_id, role_id).await
    }

    /// Return all enabled API permission codes assigned to one role.
    pub async fn get_role_perm_codes(
        &self,
        actor: &ActorContext,
        role_id: i64,
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        self.role_repo
            .find_by_id(db, tenant_id, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        let mut codes: Vec<String> = self
            .perm_repo
            .find_role_perms(db, tenant_id, &[role_id])
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect();
        codes.sort();
        codes.dedup();
        Ok(codes)
    }

    /// Atomically replace a role's data-scope mode and custom departments.
    pub async fn replace_data_scope(
        &self,
        actor: &ActorContext,
        role_id: i64,
        data_scope: &str,
        dept_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        Self::validate_data_scope(data_scope)?;
        self.get_role_model(actor, role_id).await?;

        let unique_dept_ids = if data_scope == role::Model::DATA_SCOPE_CUSTOM {
            let mut unique_dept_ids = dept_ids;
            unique_dept_ids.sort_unstable();
            unique_dept_ids.dedup();
            if unique_dept_ids.is_empty() {
                return Err(AppError::Validation(
                    "自定义数据权限至少需要一个部门".into(),
                ));
            }
            let existing_depts = self
                .dept_repo
                .find_filtered_by_ids(db, tenant_id, None, None, &unique_dept_ids)
                .await?;
            if existing_depts.len() != unique_dept_ids.len() {
                return Err(AppError::Validation(
                    "自定义数据权限包含不存在或跨租户的部门".into(),
                ));
            }
            unique_dept_ids
        } else {
            if !dept_ids.is_empty() {
                return Err(AppError::Validation("非自定义数据权限不能携带部门".into()));
            }
            Vec::new()
        };

        self.role_repo
            .replace_data_scope(db, tenant_id, role_id, data_scope, &unique_dept_ids)
            .await?;
        self.invalidate_role_users(tenant_id, role_id).await
    }
}

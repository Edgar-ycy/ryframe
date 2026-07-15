use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    cache::clear_user_permission_cache,
    repository::{PageQuery, PageResult},
};
use ryframe_db::{
    PermissionRepository, RoleRepository,
    entities::{dept, role},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Serialize;

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

pub struct RoleServiceImpl {
    pub role_repo: LoggedRepo<RoleRepository>,
    pub perm_repo: LoggedRepo<PermissionRepository>,
    pub redis: Option<RedisClient>,
}

impl RoleServiceImpl {
    fn validate_data_scope(data_scope: &str) -> AppResult<()> {
        if matches!(data_scope, "1" | "2" | "3" | "4" | "5") {
            Ok(())
        } else {
            Err(AppError::Validation("无效的数据范围值".into()))
        }
    }

    async fn invalidate_role_users(&self, db: &DatabaseConnection, role_id: i64) -> AppResult<()> {
        let user_ids = self
            .role_repo
            .find_user_ids_by_role_ids(db, &[role_id])
            .await?;
        if let Some(redis) = &self.redis {
            let tenant_id = ryframe_core::current_tenant_id();
            for user_id in user_ids {
                if let Err(error) = clear_user_permission_cache(redis, &tenant_id, user_id).await {
                    tracing::warn!(role_id, user_id, %error, "failed to clear role user permission cache");
                }
            }
        }
        Ok(())
    }

    pub async fn get_role_model(&self, db: &DatabaseConnection, id: i64) -> AppResult<role::Model> {
        self.role_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<RoleVo>> {
        let page = self
            .role_repo
            .find_by_page_filtered(db, query.clone(), name, code, status)
            .await?;
        let records = page.records.into_iter().map(RoleVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 批量删除角色
    pub async fn delete_many(&self, db: &DatabaseConnection, ids: &[i64]) -> AppResult<u64> {
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的角色".into()));
        }
        for id in ids {
            self.get_role_model(db, *id).await?;
        }
        let affected = self.role_repo.delete_many(db, ids).await?;
        for role_id in ids {
            self.invalidate_role_users(db, *role_id).await?;
        }
        Ok(affected)
    }

    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<RoleVo>> {
        let page = self.role_repo.find_by_page(db, query.clone()).await?;
        let records = page.records.into_iter().map(RoleVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<RoleVo>> {
        match self.role_repo.find_by_id(db, id).await? {
            Some(r) => {
                let mut vo = RoleVo::from(r);
                // 如果是自定义数据权限，查出关联的部门ID列表
                if vo.data_scope == "2" {
                    let dept_ids = self.role_repo.find_role_dept_ids(db, id).await?;
                    vo.dept_ids = Some(dept_ids.iter().map(|d| d.to_string()).collect());
                }
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    pub async fn get_super_role(&self, db: &DatabaseConnection) -> AppResult<role::Model> {
        self.role_repo
            .find_super_role(db)
            .await?
            .ok_or_else(|| AppError::NotFound("超级管理员角色不存在".into()))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        code: &str,
        sort: i32,
        data_scope: Option<String>,
    ) -> AppResult<RoleVo> {
        ryframe_db::TenantRepository
            .ensure_role_quota(db, &ryframe_core::current_tenant_id())
            .await?;
        if self.role_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("角色编码已存在".into()));
        }

        let data_scope = data_scope.unwrap_or_else(|| "1".to_string());
        Self::validate_data_scope(&data_scope)?;
        let mut new_role = role::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
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

        let saved = self.role_repo.insert(db, new_role).await?;
        Ok(RoleVo::from(saved))
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        sort: i32,
        status: String,
        data_scope: Option<String>,
    ) -> AppResult<RoleVo> {
        let mut role = self.get_role_model(db, id).await?;

        role.name = name.to_string();
        role.sort = sort;
        role.status = status;
        if let Some(ds) = data_scope {
            Self::validate_data_scope(&ds)?;
            role.data_scope = ds;
        }
        role.fill_on_update(&FillContext::new());

        let saved = self.role_repo.update(db, role).await?;
        self.invalidate_role_users(db, id).await?;
        Ok(RoleVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.get_role_model(db, id).await?;
        self.role_repo.delete(db, id).await?;
        self.invalidate_role_users(db, id).await
    }

    pub async fn assign_permissions(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        perm_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.get_role_model(db, role_id).await?;
        for perm_id in &perm_ids {
            self.perm_repo
                .find_by_id(db, *perm_id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("权限不存在: {}", perm_id)))?;
        }
        self.perm_repo.assign_perms(db, role_id, &perm_ids).await?;
        self.invalidate_role_users(db, role_id).await
    }

    /// Return all enabled API permission codes assigned to one role.
    pub async fn get_role_perm_codes(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
    ) -> AppResult<Vec<String>> {
        self.role_repo
            .find_by_id(db, role_id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        let mut codes: Vec<String> = self
            .perm_repo
            .find_role_perms(db, &[role_id])
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect();
        codes.sort();
        codes.dedup();
        Ok(codes)
    }

    /// Replace the custom departments assigned to one role.
    pub async fn assign_depts(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        dept_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.get_role_model(db, role_id).await?;
        let mut unique_dept_ids = dept_ids;
        unique_dept_ids.sort_unstable();
        unique_dept_ids.dedup();
        let count = dept::Entity::find()
            .filter(dept::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .filter(dept::Column::Id.is_in(unique_dept_ids.iter().copied()))
            .count(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if count != unique_dept_ids.len() as u64 {
            return Err(AppError::Validation(
                "自定义数据权限包含不存在或跨租户的部门".into(),
            ));
        }
        self.role_repo
            .assign_data_scope_depts(db, role_id, &unique_dept_ids)
            .await?;
        self.invalidate_role_users(db, role_id).await
    }

    /// Update only the role's data-scope mode.
    pub async fn update_data_scope(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        data_scope: &str,
    ) -> AppResult<()> {
        Self::validate_data_scope(data_scope)?;
        self.get_role_model(db, role_id).await?;
        self.role_repo
            .update_data_scope(db, role_id, data_scope)
            .await?;
        if data_scope != role::Model::DATA_SCOPE_CUSTOM {
            self.role_repo
                .assign_data_scope_depts(db, role_id, &[])
                .await?;
        }
        self.invalidate_role_users(db, role_id).await
    }
}

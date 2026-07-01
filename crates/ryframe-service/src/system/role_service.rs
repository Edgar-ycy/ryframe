use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
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
}

impl RoleServiceImpl {
    async fn ensure_not_admin_role(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<role::Model> {
        let role = self
            .role_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        if role.code == "admin" {
            return Err(AppError::Authorization("禁止操作超级管理员角色".into()));
        }
        Ok(role)
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
            self.ensure_not_admin_role(db, *id).await?;
        }
        self.role_repo.delete_many(db, ids).await
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

        let mut new_role = role::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
            name: name.to_string(),
            code: code.to_string(),
            data_scope: data_scope.unwrap_or_else(|| "1".to_string()),
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
        let mut role = self.ensure_not_admin_role(db, id).await?;

        role.name = name.to_string();
        role.sort = sort;
        role.status = status;
        if let Some(ds) = data_scope {
            role.data_scope = ds;
        }
        role.fill_on_update(&FillContext::new());

        let saved = self.role_repo.update(db, role).await?;
        Ok(RoleVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.ensure_not_admin_role(db, id).await?;
        self.role_repo.delete(db, id).await
    }

    pub async fn assign_permissions(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        perm_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.ensure_not_admin_role(db, role_id).await?;
        for perm_id in &perm_ids {
            self.perm_repo
                .find_by_id(db, *perm_id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("权限不存在: {}", perm_id)))?;
        }
        self.perm_repo.assign_perms(db, role_id, &perm_ids).await
    }

    /// 设置角色数据权限
    ///
    /// - `data_scope`: "1"全部 "2"自定义 "3"本部门 "4"本部门及以下 "5"仅本人
    /// - `dept_ids`: 当 data_scope="2" 时传入自定义部门ID列表
    pub async fn assign_data_scope(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        data_scope: &str,
        dept_ids: Vec<i64>,
    ) -> AppResult<()> {
        // 校验 data_scope 值
        match data_scope {
            "1" | "2" | "3" | "4" | "5" => {}
            _ => return Err(AppError::Validation("无效的数据范围值".into())),
        }

        self.ensure_not_admin_role(db, role_id).await?;

        let mut unique_dept_ids = dept_ids;
        unique_dept_ids.sort_unstable();
        unique_dept_ids.dedup();
        if data_scope == role::Model::DATA_SCOPE_CUSTOM {
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
        }

        self.role_repo
            .replace_data_scope(db, role_id, data_scope, &unique_dept_ids)
            .await
    }
}

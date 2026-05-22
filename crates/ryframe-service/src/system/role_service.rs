use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::entities::role;
use ryframe_db::{MenuRepository, PermissionRepository, RoleRepository};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;

#[derive(Debug, Serialize)]
pub struct RoleVo {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 自定义数据权限的部门ID列表（仅查询详情时填充）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dept_ids: Option<Vec<i64>>,
}

impl From<role::Model> for RoleVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id,
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
    pub role_repo: RoleRepository,
    pub perm_repo: PermissionRepository,
    pub menu_repo: MenuRepository,
}

impl RoleServiceImpl {
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
                    vo.dept_ids = Some(dept_ids);
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
        if self.role_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("角色编码已存在".into()));
        }

        let now = chrono::Utc::now();
        let new_role = role::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            code: code.to_string(),
            data_scope: data_scope.unwrap_or_else(|| "1".to_string()),
            status: "1".to_string(),
            sort,
            remark: None,
            created_at: now,
            updated_at: now,
        };

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
        let mut role = self.role_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;

        role.name = name.to_string();
        role.sort = sort;
        role.status = status;
        if let Some(ds) = data_scope {
            role.data_scope = ds;
        }
        role.updated_at = chrono::Utc::now();

        let saved = self.role_repo.update(db, role).await?;
        Ok(RoleVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.role_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        self.role_repo.delete(db, id).await
    }

    pub async fn assign_permissions(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        perm_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.role_repo.find_by_id(db, role_id).await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;
        self.perm_repo.assign_perms(db, role_id, &perm_ids).await
    }

    pub async fn assign_menus(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        menu_ids: Vec<i64>,
    ) -> AppResult<()> {
        // 使用交易进行 assign
        use ryframe_db::entities::role_menu;
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};

        self.role_repo.find_by_id(db, role_id).await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;

        let txn = db.begin().await.map_err(|e| AppError::Database(e.to_string()))?;

        // 删除旧关联
        role_menu::Entity::delete_many()
            .filter(role_menu::Column::RoleId.eq(role_id))
            .exec(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        // 插入新关联
        for menu_id in menu_ids {
            let rm = role_menu::ActiveModel {
                role_id: sea_orm::ActiveValue::Set(role_id),
                menu_id: sea_orm::ActiveValue::Set(menu_id),
            };
            rm.insert(&txn).await.map_err(|e| AppError::Database(e.to_string()))?;
        }

        txn.commit().await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
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

        self.role_repo.find_by_id(db, role_id).await?
            .ok_or_else(|| AppError::NotFound("角色不存在".into()))?;

        // 更新 data_scope 字段
        self.role_repo.update_data_scope(db, role_id, data_scope).await?;

        // 如果是自定义权限，更新关联部门
        if data_scope == "2" {
            self.role_repo.assign_data_scope_depts(db, role_id, &dept_ids).await?;
        } else {
            // 非自定义权限，清除旧的自定义部门关联
            self.role_repo.assign_data_scope_depts(db, role_id, &[]).await?;
        }

        Ok(())
    }
}

use ryframe_common::{AppError, AppResult};
use ryframe_db::entities::menu;
use ryframe_db::MenuRepository;
use ryframe_db::repositories::menu_repo::MenuTreeNode;
use sea_orm::DatabaseConnection;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;

pub struct MenuServiceImpl {
    pub menu_repo: MenuRepository,
}

impl MenuServiceImpl {
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        self.menu_repo.find_tree(db).await
    }

    pub async fn find_by_role(
        &self,
        db: &DatabaseConnection,
        role_ids: &[i64],
    ) -> AppResult<Vec<menu::Model>> {
        self.menu_repo.find_by_role_ids(db, role_ids).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        parent_id: Option<i64>,
        path: Option<&str>,
        component: Option<&str>,
        icon: Option<&str>,
        sort: i32,
        visible: bool,
    ) -> AppResult<menu::Model> {
        let now = chrono::Utc::now();
        let new_menu = menu::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            parent_id,
            path: path.map(|s| s.to_string()),
            component: component.map(|s| s.to_string()),
            icon: icon.map(|s| s.to_string()),
            sort,
            visible,
            status: menu::Model::STATUS_NORMAL.to_string(),
            del_flag: menu::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: now,
            updated_at: now,
        };
        self.menu_repo.insert(db, new_menu).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        path: Option<&str>,
        component: Option<&str>,
        icon: Option<&str>,
        sort: i32,
        visible: bool,
        status: String,
    ) -> AppResult<menu::Model> {
        let mut menu = self.menu_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;

        menu.name = name.to_string();
        menu.parent_id = parent_id;
        menu.path = path.map(|s| s.to_string());
        menu.component = component.map(|s| s.to_string());
        menu.icon = icon.map(|s| s.to_string());
        menu.sort = sort;
        menu.visible = visible;
        menu.status = status;
        menu.updated_at = chrono::Utc::now();

        self.menu_repo.update(db, menu).await
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.menu_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        self.menu_repo.delete(db, id).await
    }

    /// 按名称/状态搜索菜单列表（返回平铺列表）
    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<menu::Model>> {
        self.menu_repo.find_filtered(db, name, status).await
    }

    /// 按 ID 查询菜单详情
    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<menu::Model>> {
        self.menu_repo.find_by_id(db, id).await
    }
}


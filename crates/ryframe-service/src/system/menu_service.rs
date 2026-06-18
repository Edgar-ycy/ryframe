use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{MenuRepository, entities::menu, repositories::menu_repo::MenuTreeNode};
use sea_orm::DatabaseConnection;

const MENU_TREE_CACHE_KEY: &str = "sys_menu:tree";
const CACHE_TTL_SECS: u64 = 3600;

pub struct MenuServiceImpl {
    pub menu_repo: LoggedRepo<MenuRepository>,
    pub redis: Option<RedisClient>,
}

impl MenuServiceImpl {
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(MENU_TREE_CACHE_KEY).await
            && let Ok(cached) = serde_json::from_str::<Vec<MenuTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self.menu_repo.find_tree(db).await?;

        if let Some(ref redis) = self.redis
            && let Ok(json) = serde_json::to_string(&tree)
        {
            let _ = redis
                .set_ex(MENU_TREE_CACHE_KEY, &json, CACHE_TTL_SECS)
                .await;
        }

        Ok(tree)
    }

    pub async fn find_tree_by_permissions(
        &self,
        db: &DatabaseConnection,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        self.menu_repo
            .find_tree_by_permission_codes(db, permission_codes)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        parent_id: Option<i64>,
        menu_type: &str,
        icon: Option<&str>,
        sort: i32,
        visible: bool,
    ) -> AppResult<menu::Model> {
        let mut new_menu = menu::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: "system".to_string(),
            name: name.to_string(),
            parent_id,
            menu_type: menu_type.to_string(),
            icon: icon.map(str::to_string),
            sort,
            visible,
            status: menu::Model::STATUS_NORMAL.to_string(),
            remark: None,
            del_flag: menu::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };

        new_menu.fill_on_insert(&FillContext::new());
        self.menu_repo.insert(db, new_menu).await.inspect(|_| {
            self.invalidate_menu_cache();
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        menu_type: &str,
        icon: Option<&str>,
        sort: i32,
        visible: bool,
        status: String,
    ) -> AppResult<menu::Model> {
        let mut menu = self
            .menu_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;

        menu.name = name.to_string();
        menu.parent_id = parent_id;
        menu.menu_type = menu_type.to_string();
        menu.icon = icon.map(str::to_string);
        menu.sort = sort;
        menu.visible = visible;
        menu.status = status;
        menu.fill_on_update(&FillContext::new());

        self.menu_repo.update(db, menu).await.inspect(|_| {
            self.invalidate_menu_cache();
        })
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.menu_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;

        self.menu_repo.delete(db, id).await.map(|_| {
            self.invalidate_menu_cache();
        })
    }

    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<menu::Model>> {
        self.menu_repo.find_filtered(db, name, status).await
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<menu::Model>> {
        self.menu_repo.find_by_id(db, id).await
    }

    pub fn invalidate_menu_cache(&self) {
        if let Some(ref redis) = self.redis {
            let redis = redis.clone();
            tokio::spawn(async move {
                let _ = redis.del(MENU_TREE_CACHE_KEY).await;
            });
        }
    }
}

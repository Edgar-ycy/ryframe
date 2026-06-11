use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::entities::menu;

/// 菜单树节点
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MenuTreeNode {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub query: Option<String>,
    pub perms: Option<String>,
    pub icon: Option<String>,
    pub is_frame: bool,
    pub is_cache: bool,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub children: Vec<MenuTreeNode>,
}

pub struct MenuRepository;

#[async_trait]
impl Repository<menu::Model, i64> for MenuRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<menu::Model>> {
        menu::Entity::find_by_id(id)
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<menu::Model>> {
        crate::pagination::paginate(
            db,
            menu::Entity::find().filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL)),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        let active: menu::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        let active: menu::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = menu::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(menu::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl MenuRepository {
    /// 查询菜单树
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        let all = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(build_menu_tree(&all, None))
    }

    /// 按角色查询菜单
    pub async fn find_by_role_ids(
        &self,
        db: &DatabaseConnection,
        role_ids: &[i64],
    ) -> AppResult<Vec<menu::Model>> {
        use crate::entities::role_menu;
        let menu_ids: Vec<i64> = role_menu::Entity::find()
            .filter(role_menu::Column::RoleId.is_in(role_ids.iter().copied()))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rm| rm.menu_id)
            .collect();

        if menu_ids.is_empty() {
            return Ok(vec![]);
        }

        menu::Entity::find()
            .filter(menu::Column::Id.is_in(menu_ids))
            .filter(menu::Column::Status.eq(menu::Model::STATUS_NORMAL))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
    /// 带搜索条件的查询（返回全部，用于列表/搜索）
    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<menu::Model>> {
        let mut select =
            menu::Entity::find().filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL));
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            select = select.filter(menu::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(menu::Column::Status.eq(s));
        }
        select = select.order_by_asc(menu::Column::Sort);
        select
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}

fn build_menu_tree(menus: &[menu::Model], parent_id: Option<i64>) -> Vec<MenuTreeNode> {
    menus
        .iter()
        .filter(|m| m.parent_id == parent_id)
        .map(|m| MenuTreeNode {
            id: m.id.to_string(),
            name: m.name.clone(),
            parent_id: m.parent_id.map(|p| p.to_string()),
            menu_type: m.menu_type.clone(),
            path: m.path.clone(),
            component: m.component.clone(),
            query: m.query.clone(),
            perms: m.perms.clone(),
            icon: m.icon.clone(),
            is_frame: m.is_frame,
            is_cache: m.is_cache,
            sort: m.sort,
            visible: m.visible,
            status: m.status.clone(),
            children: build_menu_tree(menus, Some(m.id)),
        })
        .collect()
}

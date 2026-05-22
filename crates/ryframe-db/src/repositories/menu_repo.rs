use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::menu;

/// 菜单树节点
#[derive(Debug, serde::Serialize)]
pub struct MenuTreeNode {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub path: Option<String>,
    pub component: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub children: Vec<MenuTreeNode>,
}

pub struct MenuRepository;

#[async_trait]
impl Repository<menu::Model, i64> for MenuRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<menu::Model>> {
        menu::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<menu::Model>> {
        crate::pagination::paginate(db, menu::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        let active: menu::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        let active: menu::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        menu::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl MenuRepository {
    /// 查询菜单树
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        let all = menu::Entity::find()
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(build_menu_tree(&all, None))
    }

    /// 按角色查询菜单
    pub async fn find_by_role_ids(&self, db: &DatabaseConnection, role_ids: &[i64]) -> AppResult<Vec<menu::Model>> {
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
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}

fn build_menu_tree(menus: &[menu::Model], parent_id: Option<i64>) -> Vec<MenuTreeNode> {
    menus.iter()
        .filter(|m| m.parent_id == parent_id)
        .map(|m| MenuTreeNode {
            id: m.id,
            name: m.name.clone(),
            parent_id: m.parent_id,
            path: m.path.clone(),
            component: m.component.clone(),
            icon: m.icon.clone(),
            sort: m.sort,
            visible: m.visible,
            status: m.status.clone(),
            children: build_menu_tree(menus, Some(m.id)),
        })
        .collect()
}

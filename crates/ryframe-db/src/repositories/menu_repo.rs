use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::menu;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MenuTreeNode {
    /// String avoids precision loss for Snowflake IDs in JavaScript.
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub children: Vec<MenuTreeNode>,
}

pub struct MenuRepository;

const MENU_PERMISSION_CODES: &[(i64, &str)] = &[
    (4, "system:user:list"),
    (5, "system:role:list"),
    (6, "system:menu:list"),
    (7, "system:dept:list"),
    (8, "system:post:list"),
    (9, "system:dict:list"),
    (10, "system:config:list"),
    (11, "system:notice:list"),
    (12, "system:operlog:list"),
    (13, "system:logininfor:list"),
    (14, "monitor:runtime:list"),
    (15, "monitor:online:list"),
    (16, "monitor:server:list"),
    (17, "tools:gen:list"),
    (18, "system:user:list"),
    (19, "system:user:add"),
    (20, "system:user:edit"),
    (21, "system:user:remove"),
    (22, "system:user:export"),
    (23, "monitor:cache:list"),
    (24, "monitor:db-pool:list"),
    (25, "system:permission:list"),
];

#[async_trait]
impl Repository<menu::Model, i64> for MenuRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<menu::Model>> {
        menu::Entity::find_by_id(id)
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
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
            menu::Entity::find()
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id())),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        insert_entity!(menu, db, entity)
    }

    async fn update(&self, db: &DatabaseConnection, entity: menu::Model) -> AppResult<menu::Model> {
        update_entity!(menu, db, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        soft_delete_entity!(menu, db, id)
    }
}

impl MenuRepository {
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        let all = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(build_menu_tree(&all, None))
    }

    pub async fn find_by_permission_codes(
        &self,
        db: &DatabaseConnection,
        permission_codes: &[String],
    ) -> AppResult<Vec<menu::Model>> {
        if permission_codes.is_empty() {
            return Ok(vec![]);
        }

        let all = menu::Entity::find()
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(menu::Column::Status.eq(menu::Model::STATUS_NORMAL))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let permission_set: HashSet<&str> = permission_codes.iter().map(String::as_str).collect();
        if permission_set.contains("*:*:*") {
            return Ok(all);
        }

        let by_id: HashMap<i64, &menu::Model> = all.iter().map(|menu| (menu.id, menu)).collect();
        let mut visible_ids = HashSet::new();

        for item in &all {
            let Some(permission_code) = menu_permission_code(item.id) else {
                continue;
            };
            if !permission_set.contains(permission_code) {
                continue;
            }

            let mut current_id = Some(item.id);
            while let Some(id) = current_id {
                if !visible_ids.insert(id) {
                    break;
                }
                current_id = by_id.get(&id).and_then(|item| item.parent_id);
            }
        }

        Ok(all
            .into_iter()
            .filter(|item| visible_ids.contains(&item.id))
            .collect())
    }

    pub async fn find_tree_by_permission_codes(
        &self,
        db: &DatabaseConnection,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let menus = self.find_by_permission_codes(db, permission_codes).await?;
        Ok(build_menu_tree(&menus, None))
    }

    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<menu::Model>> {
        let mut select = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()));

        if let Some(n) = name.filter(|n| !n.is_empty()) {
            select = select.filter(menu::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(menu::Column::Status.eq(s));
        }

        select
            .order_by_asc(menu::Column::Sort)
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
            icon: m.icon.clone(),
            sort: m.sort,
            visible: m.visible,
            status: m.status.clone(),
            children: build_menu_tree(menus, Some(m.id)),
        })
        .collect()
}

fn menu_permission_code(menu_id: i64) -> Option<&'static str> {
    MENU_PERMISSION_CODES
        .iter()
        .find_map(|(id, code)| (*id == menu_id).then_some(*code))
}

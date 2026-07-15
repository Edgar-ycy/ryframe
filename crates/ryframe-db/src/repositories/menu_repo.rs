use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::{menu, permission};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MenuTreeNode {
    /// String avoids precision loss for Snowflake IDs in JavaScript.
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub perm_id: Option<String>,
    pub perm_code: Option<String>,
    pub route_key: Option<String>,
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
    pub async fn find_by_route_key(
        &self,
        db: &DatabaseConnection,
        route_key: &str,
    ) -> AppResult<Option<menu::Model>> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::RouteKey.eq(route_key))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn has_children(&self, db: &DatabaseConnection, id: i64) -> AppResult<bool> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::ParentId.eq(id))
            .one(db)
            .await
            .map(|row| row.is_some())
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        let all = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let permission_codes = self.permission_code_map(db, &all).await?;
        Ok(build_menu_tree(&all, None, &permission_codes))
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
            return Ok(all
                .into_iter()
                .filter(|item| item.menu_type != menu::Model::MENU_TYPE_BUTTON)
                .collect());
        }

        let by_id: HashMap<i64, &menu::Model> = all.iter().map(|menu| (menu.id, menu)).collect();
        let menu_permission_codes = self.permission_code_map(db, &all).await?;
        let mut visible_ids = HashSet::new();

        // Only a page menu's own permission may grant access to that page.
        // Button permissions control actions inside an already accessible page;
        // they must never promote the parent page into the navigation tree.
        for item in all
            .iter()
            .filter(|item| item.menu_type == menu::Model::MENU_TYPE_MENU)
        {
            let Some(permission_code) = item
                .perm_id
                .and_then(|perm_id| menu_permission_codes.get(&perm_id))
            else {
                continue;
            };
            if !permission_set.contains(permission_code.as_str()) {
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
            .filter(|item| {
                item.menu_type != menu::Model::MENU_TYPE_BUTTON && visible_ids.contains(&item.id)
            })
            .collect())
    }

    pub async fn find_tree_by_permission_codes(
        &self,
        db: &DatabaseConnection,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let menus = self.find_by_permission_codes(db, permission_codes).await?;
        let menu_permission_codes = self.permission_code_map(db, &menus).await?;
        Ok(build_menu_tree(&menus, None, &menu_permission_codes))
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

    async fn permission_code_map(
        &self,
        db: &DatabaseConnection,
        menus: &[menu::Model],
    ) -> AppResult<HashMap<i64, String>> {
        let perm_ids: HashSet<i64> = menus.iter().filter_map(|item| item.perm_id).collect();
        if perm_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = permission::Entity::find()
            .filter(permission::Column::Id.is_in(perm_ids))
            .filter(permission::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(permission::Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(rows.into_iter().map(|row| (row.id, row.code)).collect())
    }
}

fn build_menu_tree(
    menus: &[menu::Model],
    parent_id: Option<i64>,
    permission_codes: &HashMap<i64, String>,
) -> Vec<MenuTreeNode> {
    menus
        .iter()
        .filter(|m| m.parent_id == parent_id)
        .map(|m| MenuTreeNode {
            id: m.id.to_string(),
            name: m.name.clone(),
            parent_id: m.parent_id.map(|p| p.to_string()),
            menu_type: m.menu_type.clone(),
            perm_id: m.perm_id.map(|id| id.to_string()),
            perm_code: m
                .perm_id
                .and_then(|perm_id| permission_codes.get(&perm_id).cloned()),
            route_key: m.route_key.clone(),
            icon: m.icon.clone(),
            sort: m.sort,
            visible: m.visible,
            status: m.status.clone(),
            children: build_menu_tree(menus, Some(m.id), permission_codes),
        })
        .collect()
}

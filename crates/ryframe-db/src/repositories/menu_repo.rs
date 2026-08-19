use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
};

use crate::entities::{menu, permission};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MenuTreeNode {
    /// 使用字符串可避免 Snowflake ID 在 JavaScript 中丢失精度。
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

#[derive(Debug, Default)]
pub struct MenuFilter<'a> {
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[async_trait]
impl Repository<menu::Model, i64> for MenuRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<menu::Model>> {
        menu::Entity::find_by_id(id)
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<menu::Model>> {
        crate::pagination::paginate(
            db,
            menu::Entity::find()
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::TenantId.eq(tenant_id)),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: menu::Model,
    ) -> AppResult<menu::Model> {
        insert_entity!(menu, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: menu::Model,
    ) -> AppResult<menu::Model> {
        update_entity!(menu, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(menu, db, tenant_id, id)
    }
}

impl MenuRepository {
    /// 查询诊断页面需要的完整菜单目录，包含停用节点但排除软删除记录。
    pub async fn find_all_for_diagnostics(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Vec<menu::Model>> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Sort)
            .order_by_asc(menu::Column::Id)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn find_by_id_for_update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<menu::Model>> {
        menu::Entity::find_by_id(id)
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn insert_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: menu::Model,
    ) -> AppResult<menu::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("菜单租户不匹配".into()));
        }
        menu::ActiveModel::from(entity)
            .insert(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: menu::Model,
    ) -> AppResult<menu::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("菜单租户不匹配".into()));
        }
        menu::ActiveModel::from(entity)
            .reset_all()
            .update(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn delete_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<()> {
        let result = menu::Entity::update_many()
            .col_expr(
                menu::Column::DelFlag,
                Expr::value(menu::Model::DEL_FLAG_DELETED),
            )
            .col_expr(menu::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(menu::Column::Id.eq(id))
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("菜单不存在".into()));
        }
        Ok(())
    }

    pub async fn find_by_route_key(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        route_key: &str,
    ) -> AppResult<Option<menu::Model>> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::RouteKey.eq(route_key))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn has_children(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<bool> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::ParentId.eq(id))
            .one(db)
            .await
            .map(|row| row.is_some())
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_tree(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Vec<MenuTreeNode>> {
        let all = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(tenant_id))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let permission_codes = self.permission_code_map(db, tenant_id, &all).await?;
        Ok(build_menu_tree(&all, None, &permission_codes))
    }

    pub async fn find_by_permission_codes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        permission_codes: &[String],
    ) -> AppResult<Vec<menu::Model>> {
        self.find_by_permission_codes_excluding_routes(db, tenant_id, permission_codes, &[])
            .await
    }

    /// 先移除未开通 capability 的路由，再执行 RBAC 页面筛选和祖先目录回填。
    pub async fn find_by_permission_codes_excluding_routes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        permission_codes: &[String],
        excluded_route_keys: &[String],
    ) -> AppResult<Vec<menu::Model>> {
        if permission_codes.is_empty() {
            return Ok(vec![]);
        }

        let excluded_route_keys = excluded_route_keys
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let all = menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::Status.eq(menu::Model::STATUS_NORMAL))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .filter(|item| {
                item.route_key
                    .as_deref()
                    .is_none_or(|route_key| !excluded_route_keys.contains(route_key))
            })
            .collect::<Vec<_>>();

        let permission_set: HashSet<&str> = permission_codes.iter().map(String::as_str).collect();
        let by_id: HashMap<i64, &menu::Model> = all.iter().map(|menu| (menu.id, menu)).collect();
        if permission_set.contains("*:*:*") {
            let mut visible_ids = HashSet::new();
            for item in all
                .iter()
                .filter(|item| item.menu_type == menu::Model::MENU_TYPE_MENU)
            {
                include_menu_with_complete_ancestors(item.id, &by_id, &mut visible_ids);
            }
            return Ok(all
                .into_iter()
                .filter(|item| {
                    item.menu_type != menu::Model::MENU_TYPE_BUTTON
                        && visible_ids.contains(&item.id)
                })
                .collect());
        }

        let menu_permission_codes = self.permission_code_map(db, tenant_id, &all).await?;
        let mut visible_ids = HashSet::new();

        // 只有页面菜单自身的权限可以授予访问该页面的权限。按钮权限控制已可访问页面
        // 内的操作，绝不能将父页面提升到导航树中。
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

            include_menu_with_complete_ancestors(item.id, &by_id, &mut visible_ids);
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
        tenant_id: &str,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let menus = self
            .find_by_permission_codes(db, tenant_id, permission_codes)
            .await?;
        let menu_permission_codes = self.permission_code_map(db, tenant_id, &menus).await?;
        Ok(build_menu_tree(&menus, None, &menu_permission_codes))
    }

    pub async fn find_tree_by_permission_codes_excluding_routes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        permission_codes: &[String],
        excluded_route_keys: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let menus = self
            .find_by_permission_codes_excluding_routes(
                db,
                tenant_id,
                permission_codes,
                excluded_route_keys,
            )
            .await?;
        let menu_permission_codes = self.permission_code_map(db, tenant_id, &menus).await?;
        Ok(build_menu_tree(&menus, None, &menu_permission_codes))
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: &MenuFilter<'_>,
    ) -> AppResult<PageResult<menu::Model>> {
        let mut select = menu::Entity::find()
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::TenantId.eq(tenant_id));

        if let Some(n) = filter.name.filter(|n| !n.is_empty()) {
            select = select.filter(menu::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(s) = filter.status.filter(|s| !s.is_empty()) {
            select = select.filter(menu::Column::Status.eq(s));
        }

        select = select.order_by_asc(menu::Column::Sort);
        crate::pagination::paginate(db, select, query).await
    }

    async fn permission_code_map(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        menus: &[menu::Model],
    ) -> AppResult<HashMap<i64, String>> {
        let perm_ids: HashSet<i64> = menus.iter().filter_map(|item| item.perm_id).collect();
        if perm_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = permission::Entity::find()
            .filter(permission::Column::Id.is_in(perm_ids))
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(rows.into_iter().map(|row| (row.id, row.code)).collect())
    }
}

fn include_menu_with_complete_ancestors(
    menu_id: i64,
    by_id: &HashMap<i64, &menu::Model>,
    visible_ids: &mut HashSet<i64>,
) {
    let mut chain = Vec::new();
    let mut current_id = Some(menu_id);
    while let Some(id) = current_id {
        let Some(item) = by_id.get(&id) else {
            return;
        };
        chain.push(id);
        current_id = item.parent_id;
    }
    visible_ids.extend(chain);
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

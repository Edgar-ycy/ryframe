use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{
    MenuRepository,
    entities::{menu, permission},
    repositories::menu_repo::MenuTreeNode,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

const CACHE_TTL_SECS: u64 = 3600;

fn menu_tree_cache_key() -> String {
    format!("tenant:{}:sys_menu:tree", ryframe_core::current_tenant_id())
}

pub struct MenuServiceImpl {
    pub menu_repo: LoggedRepo<MenuRepository>,
    pub redis: Option<RedisClient>,
}

impl MenuServiceImpl {
    pub async fn invalidate_all_menu_caches(&self) {
        let Some(redis) = &self.redis else {
            return;
        };
        if let Ok(keys) = redis.keys("tenant:*:sys_menu:tree").await {
            for key in keys {
                let _ = redis.del(key).await;
            }
        }
    }

    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<MenuTreeNode>> {
        let cache_key = menu_tree_cache_key();
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(&cache_key).await
            && let Ok(cached) = serde_json::from_str::<Vec<MenuTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self.menu_repo.find_tree(db).await?;

        if let Some(ref redis) = self.redis
            && let Ok(json) = serde_json::to_string(&tree)
        {
            let _ = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await;
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
        perm_id: Option<i64>,
        route_key: Option<&str>,
        icon: Option<&str>,
        sort: i32,
        visible: bool,
    ) -> AppResult<menu::Model> {
        self.validate_binding(db, None, parent_id, menu_type, perm_id, route_key)
            .await?;
        let mut new_menu = menu::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
            name: name.to_string(),
            parent_id,
            menu_type: menu_type.to_string(),
            perm_id,
            route_key: route_key.map(str::to_string),
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
        perm_id: Option<i64>,
        route_key: Option<&str>,
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

        self.validate_binding(db, Some(id), parent_id, menu_type, perm_id, route_key)
            .await?;

        menu.name = name.to_string();
        menu.parent_id = parent_id;
        menu.menu_type = menu_type.to_string();
        menu.perm_id = perm_id;
        menu.route_key = route_key.map(str::to_string);
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

        if self.menu_repo.has_children(db, id).await? {
            return Err(AppError::Validation("存在子菜单，无法删除".into()));
        }

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
            let cache_key = menu_tree_cache_key();
            tokio::spawn(async move {
                let _ = redis.del(&cache_key).await;
            });
        }
    }

    async fn validate_binding(
        &self,
        db: &DatabaseConnection,
        current_id: Option<i64>,
        parent_id: Option<i64>,
        menu_type: &str,
        perm_id: Option<i64>,
        route_key: Option<&str>,
    ) -> AppResult<()> {
        if !matches!(menu_type, "M" | "C" | "F") {
            return Err(AppError::Validation("菜单类型只能是 M、C 或 F".into()));
        }
        let route_key = route_key.map(str::trim).filter(|value| !value.is_empty());
        if menu_type == menu::Model::MENU_TYPE_BUTTON {
            if perm_id.is_none() {
                return Err(AppError::Validation("按钮菜单必须关联权限".into()));
            }
            if route_key.is_some() {
                return Err(AppError::Validation("按钮菜单不能设置页面标识".into()));
            }
        } else if menu_type == menu::Model::MENU_TYPE_MENU {
            if perm_id.is_none() {
                return Err(AppError::Validation("菜单必须关联权限".into()));
            }
            if route_key.is_none() {
                return Err(AppError::Validation("菜单缺少可用的前端页面映射".into()));
            }
        }

        if let Some(perm_id) = perm_id {
            let exists = permission::Entity::find_by_id(perm_id)
                .filter(permission::Column::TenantId.eq(ryframe_core::current_tenant_id()))
                .one(db)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
            if exists.is_none() {
                return Err(AppError::Validation(
                    "关联权限不存在或不属于当前租户".into(),
                ));
            }
        }

        if let Some(route_key) = route_key
            && let Some(existing) = self.menu_repo.find_by_route_key(db, route_key).await?
            && Some(existing.id) != current_id
        {
            return Err(AppError::Conflict("页面标识已被其他菜单使用".into()));
        }

        if let Some(parent_id) = parent_id {
            if Some(parent_id) == current_id {
                return Err(AppError::Validation("菜单不能将自己设为上级".into()));
            }
            let mut cursor = Some(parent_id);
            while let Some(id) = cursor {
                let parent = self
                    .menu_repo
                    .find_by_id(db, id)
                    .await?
                    .ok_or_else(|| AppError::Validation("上级菜单不存在".into()))?;
                if Some(parent.id) == current_id {
                    return Err(AppError::Validation(
                        "不能将菜单移动到自己的后代节点".into(),
                    ));
                }
                if parent.menu_type == menu::Model::MENU_TYPE_BUTTON {
                    return Err(AppError::Validation("按钮不能作为上级菜单".into()));
                }
                cursor = parent.parent_id;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::menu_tree_cache_key;
    use ryframe_core::{TenantContext, with_tenant_context};

    #[tokio::test]
    async fn cache_key_is_tenant_scoped() {
        let first = with_tenant_context(
            TenantContext {
                tenant_id: "tenant-a".into(),
                is_admin: false,
            },
            async { menu_tree_cache_key() },
        )
        .await;
        let second = with_tenant_context(
            TenantContext {
                tenant_id: "tenant-b".into(),
                is_admin: false,
            },
            async { menu_tree_cache_key() },
        )
        .await;
        assert_eq!(first, "tenant:tenant-a:sys_menu:tree");
        assert_eq!(second, "tenant:tenant-b:sys_menu:tree");
        assert_ne!(first, second);
    }
}

use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::DatabaseCluster;
use ryframe_db::{MenuFilter, MenuRepository, PermissionRepository, entities::menu};

mod model;
mod validation;

pub use model::{CreateMenuCommand, MenuTreeNode, MenuType, MenuVo, UpdateMenuCommand};

#[derive(Debug)]
pub struct MenuListParams {
    pub page: PageQuery,
    pub name: Option<String>,
    pub status: Option<String>,
}

const CACHE_TTL_SECS: u64 = 3600;

fn menu_tree_cache_key(tenant_id: &str) -> String {
    format!("tenant:{tenant_id}:sys_menu:tree")
}

pub struct MenuService {
    db: DatabaseCluster,
    menu_repo: LoggedRepo<MenuRepository>,
    perm_repo: LoggedRepo<PermissionRepository>,
    redis: Option<RedisClient>,
}

impl MenuService {
    pub fn new(db: DatabaseCluster, redis: Option<RedisClient>) -> Self {
        Self {
            db,
            menu_repo: LoggedRepo::new(MenuRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            redis,
        }
    }

    pub async fn invalidate_all_menu_caches(&self) {
        let Some(redis) = &self.redis else {
            return;
        };
        if let Err(error) = redis.delete_by_pattern("tenant:*:sys_menu:tree").await {
            tracing::warn!(%error, "failed to clear menu tree caches");
        }
    }

    pub async fn find_tree(&self, actor: &ActorContext) -> AppResult<Vec<MenuTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        let cache_key = menu_tree_cache_key(tenant_id);
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(&cache_key).await
            && let Ok(cached) = serde_json::from_str::<Vec<MenuTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self
            .menu_repo
            .find_tree(db, tenant_id)
            .await?
            .into_iter()
            .map(MenuTreeNode::from)
            .collect::<Vec<_>>();

        if let Some(ref redis) = self.redis
            && let Ok(json) = serde_json::to_string(&tree)
            && let Err(error) = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await
        {
            tracing::warn!(tenant_id, %error, "failed to cache menu tree");
        }

        Ok(tree)
    }

    pub async fn find_tree_by_permissions(
        &self,
        actor: &ActorContext,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        self.menu_repo
            .find_tree_by_permission_codes(db, tenant_id, permission_codes)
            .await
            .map(|nodes| nodes.into_iter().map(MenuTreeNode::from).collect())
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreateMenuCommand,
    ) -> AppResult<MenuVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let route_key = normalize_route_key(command.route_key);
        self.validate_binding(
            tenant_id,
            None,
            command.parent_id,
            command.menu_type,
            command.perm_id,
            route_key.as_deref(),
        )
        .await?;
        let mut new_menu = menu::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: command.name,
            parent_id: command.parent_id,
            menu_type: command.menu_type.as_str().to_owned(),
            perm_id: command.perm_id,
            route_key,
            icon: command.icon,
            sort: command.sort,
            visible: command.visible,
            status: menu::Model::STATUS_NORMAL.to_string(),
            remark: None,
            del_flag: menu::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };

        new_menu.fill_on_insert(&FillContext::new())?;
        let saved = self.menu_repo.insert(db, tenant_id, new_menu).await?;
        self.invalidate_menu_cache(tenant_id).await;
        Ok(MenuVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdateMenuCommand,
    ) -> AppResult<MenuVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut menu = self
            .menu_repo
            .find_by_id(db, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        let route_key = normalize_route_key(command.route_key);

        self.validate_binding(
            tenant_id,
            Some(command.id),
            command.parent_id,
            command.menu_type,
            command.perm_id,
            route_key.as_deref(),
        )
        .await?;

        menu.name = command.name;
        menu.parent_id = command.parent_id;
        menu.menu_type = command.menu_type.as_str().to_owned();
        menu.perm_id = command.perm_id;
        menu.route_key = route_key;
        menu.icon = command.icon;
        menu.sort = command.sort;
        menu.visible = command.visible;
        menu.status = command.status;
        menu.fill_on_update(&FillContext::new())?;

        let saved = self.menu_repo.update(db, tenant_id, menu).await?;
        self.invalidate_menu_cache(tenant_id).await;
        Ok(MenuVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.menu_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;

        if self.menu_repo.has_children(db, tenant_id, id).await? {
            return Err(AppError::Validation("存在子菜单，无法删除".into()));
        }

        self.menu_repo.delete(db, tenant_id, id).await?;
        self.invalidate_menu_cache(tenant_id).await;
        Ok(())
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: MenuListParams,
    ) -> AppResult<PageResult<MenuVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        let page = self
            .menu_repo
            .find_by_page_filtered(
                db,
                tenant_id,
                &params.page,
                &MenuFilter {
                    name: params.name.as_deref(),
                    status: params.status.as_deref(),
                },
            )
            .await?;
        let records = page.records.into_iter().map(MenuVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<MenuVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        self.menu_repo
            .find_by_id(db, tenant_id, id)
            .await
            .map(|menu| menu.map(MenuVo::from))
    }

    pub async fn invalidate_menu_cache(&self, tenant_id: &str) {
        if let Some(redis) = &self.redis
            && let Err(error) = redis.del(menu_tree_cache_key(tenant_id)).await
        {
            tracing::warn!(tenant_id, %error, "failed to invalidate menu tree cache");
        }
    }
}

fn normalize_route_key(route_key: Option<String>) -> Option<String> {
    route_key
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::menu_tree_cache_key;

    #[test]
    fn cache_key_is_tenant_scoped() {
        let first = menu_tree_cache_key("tenant-a");
        let second = menu_tree_cache_key("tenant-b");
        assert_eq!(first, "tenant:tenant-a:sys_menu:tree");
        assert_eq!(second, "tenant:tenant-b:sys_menu:tree");
        assert_ne!(first, second);
    }
}

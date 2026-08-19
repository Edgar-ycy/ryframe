use ryframe_adapters::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{MenuFilter, MenuRepository, TenantConfigTransferRepository, entities::menu};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};

use crate::AuthorizationCache;

mod model;
mod validation;

pub use model::{CreateMenuCommand, MenuTreeNode, MenuType, MenuVo, UpdateMenuCommand};
use validation::MenuBinding;

#[derive(Debug)]
pub struct MenuListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub status: Option<String>,
}

const CACHE_TTL_SECS: u64 = 3600;
const MENU_TREE_CACHE_NAMESPACE: &str = "menu-tree";

pub struct MenuService {
    db: ControlDatabaseCluster,
    menu_repo: MenuRepository,
    authorization_cache: AuthorizationCache,
}

impl MenuService {
    pub fn new(db: ControlDatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
        Self {
            db,
            menu_repo: MenuRepository,
            authorization_cache,
        }
    }

    pub async fn find_tree(&self, actor: &ActorContext) -> AppResult<Vec<MenuTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let cache_lookup = self
            .authorization_cache
            .read_tenant_value(tenant_id, MENU_TREE_CACHE_NAMESPACE)
            .await?;
        if let Some(json) = cache_lookup
            .as_ref()
            .and_then(|lookup| lookup.value.as_deref())
            && let Ok(cached) = serde_json::from_str::<Vec<MenuTreeNode>>(json)
        {
            return Ok(cached);
        }

        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let tree = self
            .menu_repo
            .find_tree(&db, tenant_id)
            .await?
            .into_iter()
            .map(MenuTreeNode::from)
            .collect::<Vec<_>>();

        if let Some(cache_lookup) = cache_lookup {
            let json = serde_json::to_string(&tree)
                .map_err(|error| AppError::Internal(format!("序列化菜单树缓存失败: {error}")))?;
            self.authorization_cache
                .store_tenant_value(
                    tenant_id,
                    MENU_TREE_CACHE_NAMESPACE,
                    cache_lookup.tenant_authorization_epoch,
                    &json,
                    CACHE_TTL_SECS,
                )
                .await?;
        }

        Ok(tree)
    }

    pub async fn find_tree_by_permissions(
        &self,
        actor: &ActorContext,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        self.menu_repo
            .find_tree_by_permission_codes(&db, tenant_id, permission_codes)
            .await
            .map(|nodes| nodes.into_iter().map(MenuTreeNode::from).collect())
    }

    /// 会话启动使用的强一致导航查询。Capability 路由先被移除，随后才应用 RBAC。
    pub async fn find_session_tree(
        &self,
        actor: &ActorContext,
        permission_codes: &[String],
        excluded_capability_routes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.menu_repo
            .find_tree_by_permission_codes_excluding_routes(
                self.db.write(),
                tenant_id,
                permission_codes,
                excluded_capability_routes,
            )
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
        let mut new_menu = menu::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: command.name,
            parent_id: command.parent_id,
            menu_type: command.menu_type.as_str().to_owned(),
            perm_id: command.perm_id,
            route_key: route_key.clone(),
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
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.validate_binding(
            &transaction,
            tenant_id,
            MenuBinding {
                current_id: None,
                parent_id: command.parent_id,
                menu_type: command.menu_type,
                perm_id: command.perm_id,
                route_key: route_key.as_deref(),
            },
        )
        .await?;
        let saved = self
            .menu_repo
            .insert_in_transaction(&transaction, tenant_id, new_menu)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(MenuVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdateMenuCommand,
    ) -> AppResult<MenuVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let route_key = normalize_route_key(command.route_key);
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut menu = self
            .menu_repo
            .find_by_id_for_update(&transaction, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        self.validate_binding(
            &transaction,
            tenant_id,
            MenuBinding {
                current_id: Some(command.id),
                parent_id: command.parent_id,
                menu_type: command.menu_type,
                perm_id: command.perm_id,
                route_key: route_key.as_deref(),
            },
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
        let saved = self
            .menu_repo
            .update_in_transaction(&transaction, tenant_id, menu)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(MenuVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.menu_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        if menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::ParentId.eq(id))
            .lock(sea_orm::sea_query::LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some()
        {
            return Err(AppError::Validation("存在子菜单，无法删除".into()));
        }
        self.menu_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(())
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: MenuListParams,
    ) -> AppResult<PageResult<MenuVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let page = self
            .menu_repo
            .find_by_page_filtered(
                &db,
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
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        self.menu_repo
            .find_by_id(&db, tenant_id, id)
            .await
            .map(|menu| menu.map(MenuVo::from))
    }
}

fn normalize_route_key(route_key: Option<String>) -> Option<String> {
    route_key
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

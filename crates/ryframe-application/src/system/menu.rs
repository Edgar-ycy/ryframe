use std::sync::Arc;

use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};

use crate::{
    AuthorizationCache,
    ports::system::{MenuFilter, MenuReadPort, MenuRecord, MenuWritePort, MenuWriteTransaction},
};

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
const MENU_STATUS_NORMAL: &str = "1";

pub struct MenuService {
    read: Arc<dyn MenuReadPort>,
    write: Arc<dyn MenuWritePort>,
    authorization_cache: AuthorizationCache,
}

impl MenuService {
    pub fn new(
        read: Arc<dyn MenuReadPort>,
        write: Arc<dyn MenuWritePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            read,
            write,
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
        let tree = self
            .read
            .find_tree(tenant_id)
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
        self.read
            .find_tree_by_permissions(crate::validated_tenant_id(actor)?, permission_codes)
            .await
            .map(|nodes| nodes.into_iter().map(MenuTreeNode::from).collect())
    }

    /// 会话启动使用的强一致导航查询。先排除 Capability 路由，再应用 RBAC。
    pub async fn find_session_tree(
        &self,
        actor: &ActorContext,
        permission_codes: &[String],
        excluded_capability_routes: &[String],
    ) -> AppResult<Vec<MenuTreeNode>> {
        self.read
            .find_session_tree(
                crate::validated_tenant_id(actor)?,
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
        let route_key = normalize_route_key(command.route_key);
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        self.validate_binding(
            transaction.as_ref(),
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
        let saved = transaction
            .insert(
                tenant_id,
                MenuRecord {
                    id: crate::next_id()?,
                    name: command.name,
                    parent_id: command.parent_id,
                    menu_type: command.menu_type.as_str().into(),
                    perm_id: command.perm_id,
                    route_key,
                    icon: command.icon,
                    sort: command.sort,
                    visible: command.visible,
                    status: MENU_STATUS_NORMAL.into(),
                    remark: None,
                    created_at: Default::default(),
                    updated_at: Default::default(),
                },
            )
            .await?;
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdateMenuCommand,
    ) -> AppResult<MenuVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let route_key = normalize_route_key(command.route_key);
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut record = transaction
            .find_by_id_for_update(tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        self.validate_binding(
            transaction.as_ref(),
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
        record.name = command.name;
        record.parent_id = command.parent_id;
        record.menu_type = command.menu_type.as_str().into();
        record.perm_id = command.perm_id;
        record.route_key = route_key;
        record.icon = command.icon;
        record.sort = command.sort;
        record.visible = command.visible;
        record.status = command.status;
        let saved = transaction.update(tenant_id, record).await?;
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("菜单不存在".into()))?;
        if transaction.has_child_for_update(tenant_id, id).await? {
            return Err(AppError::Validation("存在子菜单，无法删除".into()));
        }
        transaction.delete(tenant_id, id).await?;
        self.commit_mutation(transaction, tenant_id).await
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: MenuListParams,
    ) -> AppResult<PageResult<MenuVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let page = self
            .read
            .find_page(
                tenant_id,
                params.page,
                MenuFilter {
                    name: params.name.as_deref(),
                    status: params.status.as_deref(),
                },
            )
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(MenuVo::from).collect(),
            page.total,
            &params.page,
        ))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<MenuVo>> {
        self.read
            .find_by_id(crate::validated_tenant_id(actor)?, id)
            .await
            .map(|record| record.map(MenuVo::from))
    }

    async fn commit_mutation(
        &self,
        transaction: Box<dyn MenuWriteTransaction>,
        tenant_id: &str,
    ) -> AppResult<()> {
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
    }
}

fn normalize_route_key(route_key: Option<String>) -> Option<String> {
    route_key
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::normalize_route_key;

    #[test]
    fn route_key_is_trimmed_and_blank_becomes_absent() {
        assert_eq!(
            normalize_route_key(Some(" user.list ".into())).as_deref(),
            Some("user.list")
        );
        assert_eq!(normalize_route_key(Some("  ".into())), None);
    }
}

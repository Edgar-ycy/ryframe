use std::sync::Arc;

use ryframe_db::{
    AutoFill, ControlDatabaseCluster, FillContext, MenuFilter as DatabaseMenuFilter,
    MenuRepository, ReadConsistency, Repository, TenantConfigTransferRepository,
    entities::{menu, permission},
    repositories::menu_repo::MenuTreeNode as DatabaseMenuTreeNode,
};
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use crate::{
    AuthorizationCache, ControlTransaction, MenuFilter, MenuReadPort, MenuRecord, MenuTreeRecord,
    MenuWritePort, MenuWriteTransaction, PersistenceFuture,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn MenuReadPort> {
    Arc::new(LegacyMenuRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn MenuWritePort> {
    Arc::new(LegacyMenuWrite {
        database,
        authorization_cache,
    })
}

struct LegacyMenuRead {
    database: ControlDatabaseCluster,
}

struct LegacyMenuWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct LegacyMenuWriteTransaction {
    transaction: sea_orm::DatabaseTransaction,
    authorization_cache: AuthorizationCache,
}

impl MenuReadPort for LegacyMenuRead {
    fn find_tree<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<MenuTreeRecord>> {
        Box::pin(async move {
            let database = self.eventual_read();
            MenuRepository
                .find_tree(&database, tenant_id)
                .await?
                .into_iter()
                .map(to_tree_record)
                .collect()
        })
    }

    fn find_tree_by_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<MenuTreeRecord>> {
        Box::pin(async move {
            let database = self.eventual_read();
            MenuRepository
                .find_tree_by_permission_codes(&database, tenant_id, permission_codes)
                .await?
                .into_iter()
                .map(to_tree_record)
                .collect()
        })
    }

    fn find_session_tree<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
        excluded_routes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<MenuTreeRecord>> {
        Box::pin(async move {
            MenuRepository
                .find_tree_by_permission_codes_excluding_routes(
                    self.database.write(),
                    tenant_id,
                    permission_codes,
                    excluded_routes,
                )
                .await?
                .into_iter()
                .map(to_tree_record)
                .collect()
        })
    }

    fn find_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: MenuFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<MenuRecord>> {
        Box::pin(async move {
            let database = self.eventual_read();
            let result = MenuRepository
                .find_by_page_filtered(
                    &database,
                    tenant_id,
                    &page,
                    &DatabaseMenuFilter {
                        name: filter.name,
                        status: filter.status,
                    },
                )
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(to_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<MenuRecord>> {
        Box::pin(async move {
            let database = self.eventual_read();
            Ok(MenuRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }
}

impl LegacyMenuRead {
    fn eventual_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Eventual)
            .connection
    }
}

impl MenuWritePort for LegacyMenuWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn MenuWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyMenuWriteTransaction {
                transaction,
                authorization_cache: self.authorization_cache.clone(),
            }) as Box<dyn MenuWriteTransaction>)
        })
    }
}

impl MenuWriteTransaction for LegacyMenuWriteTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
                .await
                .map(|_| ())
        })
    }

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<MenuRecord>> {
        Box::pin(async move {
            Ok(MenuRepository
                .find_by_id_for_update(&self.transaction, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn permission_exists_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            permission::Entity::find_by_id(id)
                .filter(permission::Column::TenantId.eq(tenant_id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map(|record| record.is_some())
                .map_err(database_error)
        })
    }

    fn find_by_route_key_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        route_key: &'a str,
    ) -> PersistenceFuture<'a, Option<MenuRecord>> {
        Box::pin(async move {
            Ok(menu::Entity::find()
                .filter(menu::Column::TenantId.eq(tenant_id))
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::RouteKey.eq(route_key))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_record))
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: MenuRecord,
    ) -> PersistenceFuture<'a, MenuRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_insert(&FillContext::new())?;
            MenuRepository
                .insert_in_transaction(&self.transaction, tenant_id, entity)
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: MenuRecord,
    ) -> PersistenceFuture<'a, MenuRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_update(&FillContext::new())?;
            MenuRepository
                .update_in_transaction(&self.transaction, tenant_id, entity)
                .await
                .map(to_record)
        })
    }

    fn has_child_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            menu::Entity::find()
                .filter(menu::Column::TenantId.eq(tenant_id))
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::ParentId.eq(id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map(|record| record.is_some())
                .map_err(database_error)
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            MenuRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32> {
        Box::pin(async move {
            self.authorization_cache
                .increment_tenant_epoch_in_transaction(&self.transaction, tenant_id)
                .await
        })
    }

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&self.transaction, tenant_id)
                .await
                .map(|_| ())
        })
    }
}

impl ControlTransaction for LegacyMenuWriteTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }
}

fn to_record(model: menu::Model) -> MenuRecord {
    MenuRecord {
        id: model.id,
        name: model.name,
        parent_id: model.parent_id,
        menu_type: model.menu_type,
        perm_id: model.perm_id,
        route_key: model.route_key,
        icon: model.icon,
        sort: model.sort,
        visible: model.visible,
        status: model.status,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: MenuRecord) -> menu::Model {
    menu::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        parent_id: record.parent_id,
        menu_type: record.menu_type,
        perm_id: record.perm_id,
        route_key: record.route_key,
        icon: record.icon,
        sort: record.sort,
        visible: record.visible,
        status: record.status,
        remark: record.remark,
        del_flag: menu::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn to_tree_record(node: DatabaseMenuTreeNode) -> AppResult<MenuTreeRecord> {
    Ok(MenuTreeRecord {
        id: parse_id(&node.id)?,
        name: node.name,
        parent_id: node.parent_id.as_deref().map(parse_id).transpose()?,
        menu_type: node.menu_type,
        perm_id: node.perm_id.as_deref().map(parse_id).transpose()?,
        perm_code: node.perm_code,
        route_key: node.route_key,
        icon: node.icon,
        sort: node.sort,
        visible: node.visible,
        status: node.status,
        children: node
            .children
            .into_iter()
            .map(to_tree_record)
            .collect::<AppResult<_>>()?,
    })
}

fn parse_id(value: &str) -> AppResult<i64> {
    value
        .parse()
        .map_err(|_| ryframe_kernel::AppError::Internal("菜单树标识无效".into()))
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

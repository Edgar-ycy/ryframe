use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    AutoFill, ControlDatabaseCluster, FillContext, MenuFilter as DatabaseMenuFilter,
    MenuRepository, ReadConsistency, Repository, TenantConfigTransferRepository,
    entities::{menu, permission},
    repositories::menu_repo::MenuTreeNode as DatabaseMenuTreeNode,
};
use async_trait::async_trait;
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        MenuFilter, MenuReadPort, MenuRecord, MenuTreeRecord, MenuWritePort, MenuWriteTransaction,
    },
};

use super::super::transaction::DatabasePortTransaction;

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn MenuReadPort> {
    Arc::new(DatabaseMenuRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn MenuWritePort> {
    Arc::new(DatabaseMenuWrite {
        database,
        authorization_cache,
    })
}

struct DatabaseMenuRead {
    database: ControlDatabaseCluster,
}

struct DatabaseMenuWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct DatabaseMenuWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
}

#[async_trait]
impl MenuReadPort for DatabaseMenuRead {
    async fn find_tree(&self, tenant_id: &str) -> AppResult<Vec<MenuTreeRecord>> {
        let database = self.eventual_read();
        MenuRepository
            .find_tree(&database, tenant_id)
            .await?
            .into_iter()
            .map(to_tree_record)
            .collect()
    }

    async fn find_tree_by_permissions(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeRecord>> {
        let database = self.eventual_read();
        MenuRepository
            .find_tree_by_permission_codes(&database, tenant_id, permission_codes)
            .await?
            .into_iter()
            .map(to_tree_record)
            .collect()
    }

    async fn find_session_tree(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
        excluded_routes: &[String],
    ) -> AppResult<Vec<MenuTreeRecord>> {
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
    }

    async fn find_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: MenuFilter<'_>,
    ) -> AppResult<PageResult<MenuRecord>> {
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
    }

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<MenuRecord>> {
        let database = self.eventual_read();
        Ok(MenuRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record))
    }
}

impl DatabaseMenuRead {
    fn eventual_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Eventual)
            .connection
    }
}

#[async_trait]
impl MenuWritePort for DatabaseMenuWrite {
    async fn begin(&self) -> AppResult<Box<dyn MenuWriteTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseMenuWriteTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
        }) as Box<dyn MenuWriteTransaction>)
    }
}

#[async_trait]
impl MenuWriteTransaction for DatabaseMenuWriteTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<MenuRecord>> {
        Ok(MenuRepository
            .find_by_id_for_update(&self.transaction, tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn permission_exists_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool> {
        permission::Entity::find_by_id(id)
            .filter(permission::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map(|record| record.is_some())
            .db()
    }

    async fn find_by_route_key_for_update(
        &self,
        tenant_id: &str,
        route_key: &str,
    ) -> AppResult<Option<MenuRecord>> {
        Ok(menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::RouteKey.eq(route_key))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_record))
    }

    async fn insert(&self, tenant_id: &str, record: MenuRecord) -> AppResult<MenuRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_insert(&FillContext::new())?;
        MenuRepository
            .insert_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn update(&self, tenant_id: &str, record: MenuRecord) -> AppResult<MenuRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_update(&FillContext::new())?;
        MenuRepository
            .update_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn has_child_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool> {
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::ParentId.eq(id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map(|record| record.is_some())
            .db()
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        MenuRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }

    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32> {
        self.authorization_cache
            .increment_tenant_epoch_in_transaction(&self.transaction, tenant_id)
            .await
    }

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl PersistenceTransaction for DatabaseMenuWriteTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => self.transaction.commit_audited().await,
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
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

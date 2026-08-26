use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    AutoFill, ControlDatabaseCluster, DeptRepository, FillContext, ReadConsistency, Repository,
    TenantConfigTransferRepository,
    entities::{dept, role_dept, user},
    repositories::dept_repo::DeptTreeNode as DatabaseDeptTreeNode,
};
use async_trait::async_trait;
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        DeptFilter, DeptReadPort, DeptRecord, DeptTreeRecord, DeptWritePort, DeptWriteTransaction,
    },
};

use super::super::transaction::DatabasePortTransaction;

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn DeptReadPort> {
    Arc::new(DatabaseDeptRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn DeptWritePort> {
    Arc::new(DatabaseDeptWrite {
        database,
        authorization_cache,
    })
}

struct DatabaseDeptRead {
    database: ControlDatabaseCluster,
}

struct DatabaseDeptWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct DatabaseDeptWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
}

#[async_trait]
impl DeptReadPort for DatabaseDeptRead {
    async fn find_child_ids(&self, tenant_id: &str, dept_id: i64) -> AppResult<Vec<i64>> {
        let database = self.strong_read();
        DeptRepository
            .find_child_dept_ids(&database, tenant_id, dept_id)
            .await
    }

    async fn find_tree(
        &self,
        tenant_id: &str,
        visible_ids: Option<&[i64]>,
    ) -> AppResult<Vec<DeptTreeRecord>> {
        let database = self.strong_read();
        let records = match visible_ids {
            Some(ids) => {
                DeptRepository
                    .find_tree_by_visible_ids(&database, tenant_id, ids)
                    .await?
            }
            None => DeptRepository.find_tree(&database, tenant_id).await?,
        };
        records.into_iter().map(to_tree_record).collect()
    }

    async fn find_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: DeptFilter<'_>,
        visible_ids: Option<&[i64]>,
    ) -> AppResult<PageResult<DeptRecord>> {
        let database = self.strong_read();
        let result = match visible_ids {
            Some(ids) => {
                DeptRepository
                    .find_by_page_filtered_by_ids(
                        &database,
                        tenant_id,
                        page,
                        filter.name,
                        filter.status,
                        ids,
                    )
                    .await?
            }
            None => {
                DeptRepository
                    .find_by_page_filtered(&database, tenant_id, page, filter.name, filter.status)
                    .await?
            }
        };
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<DeptRecord>> {
        let database = self.strong_read();
        Ok(DeptRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record))
    }
}

impl DatabaseDeptRead {
    fn strong_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Strong)
            .connection
    }
}

#[async_trait]
impl DeptWritePort for DatabaseDeptWrite {
    async fn begin(&self) -> AppResult<Box<dyn DeptWriteTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseDeptWriteTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
        }) as Box<dyn DeptWriteTransaction>)
    }
}

#[async_trait]
impl DeptWriteTransaction for DatabaseDeptWriteTransaction {
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
    ) -> AppResult<Option<DeptRecord>> {
        Ok(DeptRepository
            .find_by_id_for_update(&self.transaction, tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn find_descendants_for_update(
        &self,
        tenant_id: &str,
        old_prefix: &str,
    ) -> AppResult<Vec<DeptRecord>> {
        dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .filter(
                Condition::any()
                    .add(dept::Column::Ancestors.eq(old_prefix))
                    .add(dept::Column::Ancestors.like(format!("{old_prefix},%"))),
            )
            .order_by_asc(dept::Column::Id)
            .lock(LockType::Update)
            .all(&self.transaction)
            .await
            .map(|records| records.into_iter().map(to_record).collect())
            .db()
    }

    async fn insert(&self, tenant_id: &str, record: DeptRecord) -> AppResult<DeptRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_insert(&FillContext::new())?;
        DeptRepository
            .insert_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn update(&self, tenant_id: &str, record: DeptRecord) -> AppResult<DeptRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_update(&FillContext::new())?;
        DeptRepository
            .update_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn has_child_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool> {
        dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .filter(dept::Column::ParentId.eq(id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map(|record| record.is_some())
            .db()
    }

    async fn has_reference_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool> {
        let has_user = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::DeptId.eq(id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .is_some();
        if has_user {
            return Ok(true);
        }
        role_dept::Entity::find()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::DeptId.eq(id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map(|record| record.is_some())
            .db()
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        DeptRepository
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
impl PersistenceTransaction for DatabaseDeptWriteTransaction {
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

fn to_record(model: dept::Model) -> DeptRecord {
    DeptRecord {
        id: model.id,
        name: model.name,
        parent_id: model.parent_id,
        ancestors: model.ancestors,
        sort: model.sort,
        status: model.status,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: DeptRecord) -> dept::Model {
    dept::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        parent_id: record.parent_id,
        ancestors: record.ancestors,
        sort: record.sort,
        status: record.status,
        remark: record.remark,
        del_flag: dept::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn to_tree_record(node: DatabaseDeptTreeNode) -> ryframe_kernel::AppResult<DeptTreeRecord> {
    Ok(DeptTreeRecord {
        id: node
            .id
            .parse()
            .map_err(|_| ryframe_kernel::AppError::Internal("部门树标识无效".into()))?,
        name: node.name,
        parent_id: node
            .parent_id
            .map(|id| {
                id.parse()
                    .map_err(|_| ryframe_kernel::AppError::Internal("部门树父级标识无效".into()))
            })
            .transpose()?,
        sort: node.sort,
        status: node.status,
        children: node
            .children
            .into_iter()
            .map(to_tree_record)
            .collect::<ryframe_kernel::AppResult<_>>()?,
    })
}

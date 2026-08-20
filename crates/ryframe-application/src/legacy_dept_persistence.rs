use std::sync::Arc;

use ryframe_db::{
    AutoFill, ControlDatabaseCluster, DeptRepository, FillContext, ReadConsistency, Repository,
    TenantConfigTransferRepository,
    entities::{dept, role_dept, user},
    repositories::dept_repo::DeptTreeNode as DatabaseDeptTreeNode,
};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use crate::{
    AuthorizationCache, ControlTransaction, DeptFilter, DeptReadPort, DeptRecord, DeptTreeRecord,
    DeptWritePort, DeptWriteTransaction, PersistenceFuture,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn DeptReadPort> {
    Arc::new(LegacyDeptRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn DeptWritePort> {
    Arc::new(LegacyDeptWrite {
        database,
        authorization_cache,
    })
}

struct LegacyDeptRead {
    database: ControlDatabaseCluster,
}

struct LegacyDeptWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct LegacyDeptWriteTransaction {
    transaction: sea_orm::DatabaseTransaction,
    authorization_cache: AuthorizationCache,
}

impl DeptReadPort for LegacyDeptRead {
    fn find_child_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>> {
        Box::pin(async move {
            let database = self.strong_read();
            DeptRepository
                .find_child_dept_ids(&database, tenant_id, dept_id)
                .await
        })
    }

    fn find_tree<'a>(
        &'a self,
        tenant_id: &'a str,
        visible_ids: Option<&'a [i64]>,
    ) -> PersistenceFuture<'a, Vec<DeptTreeRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: DeptFilter<'a>,
        visible_ids: Option<&'a [i64]>,
    ) -> PersistenceFuture<'a, PageResult<DeptRecord>> {
        Box::pin(async move {
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
                        .find_by_page_filtered(
                            &database,
                            tenant_id,
                            page,
                            filter.name,
                            filter.status,
                        )
                        .await?
                }
            };
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
    ) -> PersistenceFuture<'a, Option<DeptRecord>> {
        Box::pin(async move {
            let database = self.strong_read();
            Ok(DeptRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }
}

impl LegacyDeptRead {
    fn strong_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Strong)
            .connection
    }
}

impl DeptWritePort for LegacyDeptWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn DeptWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyDeptWriteTransaction {
                transaction,
                authorization_cache: self.authorization_cache.clone(),
            }) as Box<dyn DeptWriteTransaction>)
        })
    }
}

impl DeptWriteTransaction for LegacyDeptWriteTransaction {
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
    ) -> PersistenceFuture<'a, Option<DeptRecord>> {
        Box::pin(async move {
            Ok(DeptRepository
                .find_by_id_for_update(&self.transaction, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_descendants_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        old_prefix: &'a str,
    ) -> PersistenceFuture<'a, Vec<DeptRecord>> {
        Box::pin(async move {
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
                .map_err(database_error)
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DeptRecord,
    ) -> PersistenceFuture<'a, DeptRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_insert(&FillContext::new())?;
            DeptRepository
                .insert_in_transaction(&self.transaction, tenant_id, entity)
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DeptRecord,
    ) -> PersistenceFuture<'a, DeptRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_update(&FillContext::new())?;
            DeptRepository
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
            dept::Entity::find()
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .filter(dept::Column::ParentId.eq(id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map(|record| record.is_some())
                .map_err(database_error)
        })
    }

    fn has_reference_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let has_user = user::Entity::find()
                .filter(user::Column::TenantId.eq(tenant_id))
                .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
                .filter(user::Column::DeptId.eq(id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
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
                .map_err(database_error)
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            DeptRepository
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

impl ControlTransaction for LegacyDeptWriteTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
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

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

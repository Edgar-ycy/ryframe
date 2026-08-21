use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, NoticeFilter as DatabaseNoticeFilter, NoticeRepository,
    ReadConsistency, Repository, entities::notice,
};
use ryframe_kernel::{AppError, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    ControlTransaction, PersistenceFuture,
    ports::system::{NoticeFilter, NoticePersistencePort, NoticeRecord, NoticeTransaction},
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn NoticePersistencePort> {
    Arc::new(DatabaseNoticePersistence { database })
}

struct DatabaseNoticePersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseNoticeTransaction {
    transaction: sea_orm::DatabaseTransaction,
}

impl NoticePersistencePort for DatabaseNoticePersistence {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            Ok(NoticeRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: NoticeFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<NoticeRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            let filter = DatabaseNoticeFilter {
                title: filter.title,
                notice_type: filter.notice_type,
                status: filter.status,
                data_scope: filter.data_scope,
            };
            let result = NoticeRepository
                .find_by_page_filtered(&database, tenant_id, &page, &filter)
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(to_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn NoticeTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseNoticeTransaction { transaction }) as Box<dyn NoticeTransaction>)
        })
    }
}

impl NoticeTransaction for DatabaseNoticeTransaction {
    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
        Box::pin(async move {
            Ok(notice::Entity::find_by_id(id)
                .filter(notice::Column::TenantId.eq(tenant_id))
                .filter(notice::Column::DelFlag.eq(notice::Model::DEL_FLAG_NORMAL))
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
        record: NoticeRecord,
    ) -> PersistenceFuture<'a, NoticeRecord> {
        Box::pin(async move {
            NoticeRepository
                .insert_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: NoticeRecord,
    ) -> PersistenceFuture<'a, NoticeRecord> {
        Box::pin(async move {
            NoticeRepository
                .update_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            NoticeRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }
}

impl ControlTransaction for DatabaseNoticeTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::super::audit_persistence::commit_current_audit(self.transaction).await
        })
    }
}

fn to_record(model: notice::Model) -> NoticeRecord {
    NoticeRecord {
        id: model.id,
        title: model.title,
        content: model.content,
        notice_type: model.r#type,
        status: model.status,
        created_by: model.created_by,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: NoticeRecord) -> notice::Model {
    notice::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        title: record.title,
        content: record.content,
        r#type: record.notice_type,
        status: record.status,
        created_by: record.created_by,
        del_flag: notice::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

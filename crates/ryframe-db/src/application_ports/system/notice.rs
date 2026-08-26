use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, NoticeFilter as DatabaseNoticeFilter, NoticeRepository,
    ReadConsistency, Repository, entities::notice,
};
use async_trait::async_trait;
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
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

#[async_trait]
impl NoticePersistencePort for DatabaseNoticePersistence {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<NoticeRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        Ok(NoticeRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: NoticeFilter<'_>,
    ) -> AppResult<PageResult<NoticeRecord>> {
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
    }

    async fn begin(&self) -> AppResult<Box<dyn NoticeTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseNoticeTransaction { transaction }) as Box<dyn NoticeTransaction>)
    }
}

#[async_trait]
impl NoticeTransaction for DatabaseNoticeTransaction {
    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<NoticeRecord>> {
        Ok(notice::Entity::find_by_id(id)
            .filter(notice::Column::TenantId.eq(tenant_id))
            .filter(notice::Column::DelFlag.eq(notice::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_record))
    }

    async fn insert(&self, tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord> {
        NoticeRepository
            .insert_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
            .await
            .map(to_record)
    }

    async fn update(&self, tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord> {
        NoticeRepository
            .update_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
            .await
            .map(to_record)
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        NoticeRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }
}

#[async_trait]
impl PersistenceTransaction for DatabaseNoticeTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
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

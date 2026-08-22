use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, LoginInfoFilter as DatabaseLoginInfoFilter, LoginInfoRepository,
    ReadConsistency, Repository, entities::login_info,
};
use async_trait::async_trait;
use ryframe_kernel::{
    AppResult, DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use sea_orm::TransactionTrait;

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, LoginInfoTransaction,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn LoginInfoPersistencePort> {
    Arc::new(DatabaseLoginInfoPersistence { database })
}

struct DatabaseLoginInfoPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseLoginInfoTransaction {
    transaction: sea_orm::DatabaseTransaction,
}

#[async_trait]
impl LoginInfoPersistencePort for DatabaseLoginInfoPersistence {
    async fn insert(&self, tenant_id: &str, record: LoginInfoRecord) -> AppResult<()> {
        LoginInfoRepository
            .insert(
                self.database.write(),
                tenant_id,
                to_entity(tenant_id, record),
            )
            .await
            .map(|_| ())
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContext,
    ) -> AppResult<PageResult<LoginInfoRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        let result = LoginInfoRepository
            .find_by_page_filtered(
                &database,
                tenant_id,
                &page,
                to_database_filter(filter),
                data_scope,
            )
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContext,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<LoginInfoRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        LoginInfoRepository
            .find_for_export_after_id(
                &database,
                tenant_id,
                &to_database_filter(filter),
                data_scope,
                window,
            )
            .await
            .map(|records| records.into_iter().map(to_record).collect())
    }

    async fn begin(&self) -> AppResult<Box<dyn LoginInfoTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseLoginInfoTransaction { transaction }) as Box<dyn LoginInfoTransaction>)
    }
}

#[async_trait]
impl LoginInfoTransaction for DatabaseLoginInfoTransaction {
    async fn clean(&self, tenant_id: &str) -> AppResult<u64> {
        LoginInfoRepository
            .clean_all_in_transaction(&self.transaction, tenant_id)
            .await
    }
}

#[async_trait]
impl PersistenceTransaction for DatabaseLoginInfoTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => self.transaction.commit().await.map_err(database_error),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn to_database_filter(filter: LoginInfoFilter<'_>) -> DatabaseLoginInfoFilter<'_> {
    DatabaseLoginInfoFilter {
        user_name: filter.user_name,
        status: filter.status,
        begin_time: filter.begin_time,
        end_time: filter.end_time,
    }
}

fn to_record(model: login_info::Model) -> LoginInfoRecord {
    LoginInfoRecord {
        id: model.id,
        user_name: model.user_name,
        ipaddr: model.ipaddr,
        login_location: model.login_location,
        browser: model.browser,
        os: model.os,
        status: model.status,
        message: model.msg,
        login_time: model.login_time,
    }
}

fn to_entity(tenant_id: &str, record: LoginInfoRecord) -> login_info::Model {
    login_info::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        user_name: record.user_name,
        ipaddr: record.ipaddr,
        login_location: record.login_location,
        browser: record.browser,
        os: record.os,
        status: record.status,
        msg: record.message,
        login_time: record.login_time,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

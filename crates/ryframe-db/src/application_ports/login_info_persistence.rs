use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, LoginInfoFilter as DatabaseLoginInfoFilter, LoginInfoRepository,
    ReadConsistency, Repository, entities::login_info,
};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::TransactionTrait;

use ryframe_application::{
    ControlTransaction, LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord,
    LoginInfoTransaction, PersistenceFuture,
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

impl LoginInfoPersistencePort for DatabaseLoginInfoPersistence {
    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: LoginInfoRecord,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            LoginInfoRepository
                .insert(
                    self.database.write(),
                    tenant_id,
                    to_entity(tenant_id, record),
                )
                .await
                .map(|_| ())
        })
    }

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: LoginInfoFilter<'a>,
        data_scope: &'a ryframe_kernel::DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<LoginInfoRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: LoginInfoFilter<'a>,
        data_scope: &'a ryframe_kernel::DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<LoginInfoRecord>> {
        Box::pin(async move {
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
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn LoginInfoTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseLoginInfoTransaction { transaction })
                as Box<dyn LoginInfoTransaction>)
        })
    }
}

impl LoginInfoTransaction for DatabaseLoginInfoTransaction {
    fn clean<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            LoginInfoRepository
                .clean_all_in_transaction(&self.transaction, tenant_id)
                .await
        })
    }
}

impl ControlTransaction for DatabaseLoginInfoTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(
            async move { super::audit_persistence::commit_current_audit(self.transaction).await },
        )
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

use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use ryframe_kernel::{
    AppResult, DataScopeContextRef, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use sea_orm::TransactionTrait;
use time::OffsetDateTime;

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, LoginInfoTransaction,
    },
};

use crate::{
    ControlDatabaseCluster, DbResultExt, LoginInfoFilter as DatabaseLoginInfoFilter,
    LoginInfoRepository, ReadConsistency, Repository, entities::login_info,
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
                to_entity(tenant_id, record)?,
            )
            .await
            .map(|_| ())
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContextRef<'_>,
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
        let records = result
            .records
            .into_iter()
            .map(to_record)
            .collect::<AppResult<Vec<_>>>()?;
        Ok(PageResult::new(records, result.total, &page))
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContextRef<'_>,
        window: ExportCursorWindow<'_>,
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
            .await?
            .into_iter()
            .map(to_record)
            .collect()
    }

    async fn begin(&self) -> AppResult<Box<dyn LoginInfoTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
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
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
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

fn to_record(model: login_info::Model) -> AppResult<LoginInfoRecord> {
    let login_time = OffsetDateTime::from_unix_timestamp(model.login_time.timestamp())
        .and_then(|time| time.replace_nanosecond(model.login_time.timestamp_subsec_nanos()))
        .db_context("读取登录日志 UTC 时间")?;
    Ok(LoginInfoRecord {
        id: model.id,
        user_name: model.user_name,
        ipaddr: model.ipaddr,
        login_location: model.login_location,
        browser: model.browser,
        os: model.os,
        status: model.status,
        message: model.msg,
        login_time,
    })
}

fn to_entity(tenant_id: &str, record: LoginInfoRecord) -> AppResult<login_info::Model> {
    let login_time = DateTime::from_timestamp(
        record.login_time.unix_timestamp(),
        record.login_time.nanosecond(),
    )
    .ok_or("时间超出 UTC 表示范围")
    .db_context("写入登录日志 UTC 时间")?;
    Ok(login_info::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        user_name: record.user_name,
        ipaddr: record.ipaddr,
        login_location: record.login_location,
        browser: record.browser,
        os: record.os,
        status: record.status,
        msg: record.message,
        login_time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use ryframe_kernel::AppError;

    #[test]
    fn login_time_round_trips_between_sql_and_the_application_in_utc() {
        let login_time = DateTime::from_timestamp(-1, 123_456_000).unwrap();
        let entity = login_info::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            user_name: "tester".into(),
            ipaddr: "127.0.0.1".into(),
            login_location: None,
            browser: None,
            os: None,
            status: "1".into(),
            msg: None,
            login_time,
        };
        let mut record = to_record(entity.clone()).unwrap();
        assert_eq!(record.login_time.unix_timestamp(), -1);
        assert_eq!(record.login_time.nanosecond(), 123_456_000);
        assert_eq!(record.login_time.offset(), time::UtcOffset::UTC);

        record.login_time = record
            .login_time
            .to_offset(time::UtcOffset::from_hms(8, 0, 0).unwrap());
        assert_eq!(to_entity("tenant-a", record.clone()).unwrap(), entity);

        let earlier = Utc.with_ymd_and_hms(999, 12, 31, 23, 59, 59).unwrap();
        record.login_time = OffsetDateTime::from_unix_timestamp(earlier.timestamp()).unwrap();
        assert_eq!(to_entity("tenant-a", record).unwrap().login_time, earlier);

        for login_time in [
            Utc.with_ymd_and_hms(10000, 1, 1, 0, 0, 0).unwrap(),
            DateTime::from_timestamp(-1, 1_123_456_000).unwrap(),
        ] {
            let invalid = login_info::Model {
                login_time,
                ..entity.clone()
            };
            assert!(matches!(to_record(invalid), Err(AppError::Database(_))));
        }
    }
}

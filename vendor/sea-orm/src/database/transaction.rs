#![allow(unused_assignments)]
use std::{future::Future, pin::Pin, sync::Arc};

use futures_util::lock::Mutex;
#[cfg(feature = "sqlx-sqlite")]
use sqlx_core::sql_str::SqlSafeStr;
#[cfg(feature = "sqlx-dep")]
use sqlx_core::transaction::TransactionManager;
use tracing::instrument;

use crate::{
    AccessMode, ConnectionTrait, DbBackend, DbErr, ExecResult, InnerConnection, IsolationLevel,
    QueryResult, SqliteTransactionMode, Statement, TransactionOptions, TransactionSession,
    TransactionTrait, debug_print, error::*,
};
#[cfg(feature = "sqlx-dep")]
use crate::{sqlx_error_to_exec_err, sqlx_error_to_query_err};

#[cfg(feature = "stream")]
use crate::{StreamTrait, TransactionStream};

/// A live database transaction.
///
/// Obtain one with
/// [`TransactionTrait::begin`](crate::TransactionTrait::begin) (manual
/// commit/rollback) or
/// [`TransactionTrait::transaction`](crate::TransactionTrait::transaction)
/// (auto commit on `Ok`, auto rollback on `Err`). Like
/// [`DatabaseConnection`](crate::DatabaseConnection) it implements
/// [`ConnectionTrait`](crate::ConnectionTrait), so SeaORM's query and
/// mutation methods work against it transparently. Calling `begin` on a
/// transaction starts a nested transaction via `SAVEPOINT`.
pub struct DatabaseTransaction {
    conn: Arc<Mutex<InnerConnection>>,
    backend: DbBackend,
    open: bool,
    session_invalid: bool,
    metric_callback: Option<crate::metric::Callback>,
    record_stmt_in_spans: bool,
}

#[instrument(level = "trace", skip(transaction, callback))]
pub(crate) async fn run_async_transaction_callback<Txn, F, T, E>(
    transaction: Txn,
    callback: F,
) -> Result<T, TransactionError<E>>
where
    Txn: TransactionSession + Send + Sync,
    F: for<'b> AsyncFnOnce(&'b Txn) -> Result<T, E> + Send,
    T: Send,
    E: std::fmt::Display + std::fmt::Debug + Send,
{
    let res = callback(&transaction)
        .await
        .map_err(TransactionError::Transaction);
    if res.is_ok() {
        transaction
            .commit()
            .await
            .map_err(TransactionError::Connection)?;
    } else {
        transaction
            .rollback()
            .await
            .map_err(TransactionError::Connection)?;
    }
    res
}

impl std::fmt::Debug for DatabaseTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DatabaseTransaction")
    }
}

impl DatabaseTransaction {
    #[instrument(level = "trace", skip(metric_callback))]
    pub(crate) async fn begin(
        conn: Arc<Mutex<InnerConnection>>,
        backend: DbBackend,
        metric_callback: Option<crate::metric::Callback>,
        record_stmt_in_spans: bool,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
        sqlite_transaction_mode: Option<SqliteTransactionMode>,
    ) -> Result<DatabaseTransaction, DbErr> {
        let res = DatabaseTransaction {
            conn,
            backend,
            open: true,
            session_invalid: false,
            metric_callback,
            record_stmt_in_spans,
        };

        let begin_result: Result<(), DbErr> = super::tracing_spans::with_db_span!(
            "sea_orm.begin",
            backend,
            "BEGIN",
            record_stmt = false,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *res.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *res.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(c) => {
                        // in MySQL SET TRANSACTION operations must be executed before transaction start
                        crate::driver::sqlx_mysql::set_transaction_config(
                            c,
                            isolation_level,
                            access_mode,
                        )
                        .await?;
                        <sqlx::MySql as sqlx::Database>::TransactionManager::begin(c, None)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(c) => {
                        <sqlx::Postgres as sqlx::Database>::TransactionManager::begin(c, None)
                            .await
                            .map_err(sqlx_error_to_query_err)?;
                        // in PostgreSQL SET TRANSACTION operations must be executed inside transaction
                        crate::driver::sqlx_postgres::set_transaction_config(
                            c,
                            isolation_level,
                            access_mode,
                        )
                        .await
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(c) => {
                        crate::driver::sqlx_sqlite::set_transaction_config(
                            c,
                            isolation_level,
                            access_mode,
                        )
                        .await?;
                        let depth = <sqlx::Sqlite as sqlx::Database>::TransactionManager::get_transaction_depth(c);
                        let statement = if depth == 0 {
                            sqlite_transaction_mode.map(|mode| {
                                sqlx::AssertSqlSafe(format!("BEGIN {}", mode.sqlite_keyword()))
                                    .into_sql_str()
                            })
                        } else {
                            // Nested transaction uses SAVEPOINT; the mode only applies to the top-level BEGIN
                            None
                        };
                        <sqlx::Sqlite as sqlx::Database>::TransactionManager::begin(c, statement)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(c) => c.begin(sqlite_transaction_mode),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(c) => {
                        c.begin();
                        Ok(())
                    }
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(c) => {
                        c.begin().await;
                        Ok(())
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        );

        begin_result?;
        Ok(res)
    }

    /// Runs a transaction to completion passing through the result.
    /// Rolling back the transaction on encountering an error.
    #[instrument(level = "trace", skip(callback))]
    pub(crate) async fn run<F, T, E>(self, callback: F) -> Result<T, TransactionError<E>>
    where
        F: for<'b> FnOnce(
                &'b DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'b>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let res = callback(&self).await.map_err(TransactionError::Transaction);
        if res.is_ok() {
            self.commit().await.map_err(TransactionError::Connection)?;
        } else {
            self.rollback()
                .await
                .map_err(TransactionError::Connection)?;
        }
        res
    }

    /// Execute the function inside a transaction.
    /// If the function returns an error, the transaction will be rolled back.
    /// Otherwise, the transaction will be committed.
    #[instrument(level = "trace", skip(callback))]
    pub async fn transaction_async<F, T, E>(&self, callback: F) -> Result<T, TransactionError<E>>
    where
        F: for<'c> AsyncFnOnce(&'c DatabaseTransaction) -> Result<T, E> + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let transaction = self.begin().await.map_err(TransactionError::Connection)?;
        run_async_transaction_callback(transaction, callback).await
    }

    /// Execute the function inside a transaction with isolation level and/or access mode.
    /// If the function returns an error, the transaction will be rolled back.
    /// Otherwise, the transaction will be committed.
    #[instrument(level = "trace", skip(callback))]
    pub async fn transaction_with_config_async<F, T, E>(
        &self,
        callback: F,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<T, TransactionError<E>>
    where
        F: for<'c> AsyncFnOnce(&'c DatabaseTransaction) -> Result<T, E> + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let transaction = self
            .begin_with_config(isolation_level, access_mode)
            .await
            .map_err(TransactionError::Connection)?;
        run_async_transaction_callback(transaction, callback).await
    }

    /// 当前持有者是否仍承担事务结束/回滚责任；不是服务器事务状态或命名锁证明。
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open && !self.session_invalid
    }

    fn ensure_session_usable(&self) -> Result<(), DbErr> {
        if self.session_invalid {
            Err(conn_err(
                "保留会话事务结果未知或已取消；连接已标记丢弃，禁止继续或自动重放",
            ))
        } else {
            Ok(())
        }
    }

    /// 只结束深度为一的 MySQL 根事务，保留独占物理会话及其命名锁。
    /// 开始网络操作前禁用迟到 Drop 回滚；错误或取消后该会话永久失效并丢弃连接。
    #[cfg(feature = "sqlx-mysql")]
    pub async fn finish_mysql_root(&mut self, commit: bool) -> Result<(), DbErr> {
        self.ensure_session_usable()?;
        if self.backend != DbBackend::MySql {
            return Err(conn_err("保留会话根事务仅支持 MySQL"));
        }
        if !self.open || Arc::strong_count(&self.conn) != 1 {
            return Err(conn_err(
                "结束保留会话根事务需要独占的活跃持有者，拒绝嵌套或共享事务",
            ));
        }
        #[cfg(not(feature = "sync"))]
        let mut guard = self.conn.lock().await;
        #[cfg(feature = "sync")]
        let mut guard = self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;
        match &mut *guard {
            InnerConnection::MySql(connection) => {
                if <sqlx::MySql as sqlx::Database>::TransactionManager::get_transaction_depth(
                    connection,
                ) != 1
                {
                    return Err(conn_err("finish_mysql_root 仅接受事务深度一"));
                }
                connection.close_on_drop();
                self.open = false;
                self.session_invalid = true;
                if commit {
                    <sqlx::MySql as sqlx::Database>::TransactionManager::commit(connection).await
                } else {
                    <sqlx::MySql as sqlx::Database>::TransactionManager::rollback(connection).await
                }
                .map_err(sqlx_error_to_query_err)?;
                self.session_invalid = false;
                Ok(())
            }
            #[cfg(any(
                feature = "sqlx-postgres",
                feature = "sqlx-sqlite",
                feature = "rusqlite",
                feature = "mock",
                feature = "proxy"
            ))]
            _ => Err(conn_err("保留会话根事务仅支持 MySQL")),
        }
    }

    /// 在已结束事务的独占 MySQL 会话上开启新的根事务，不租用第二连接。
    /// 深度必须为零；失败或取消后禁止继续，迟到 Drop 不会回滚下一代事务。
    #[cfg(feature = "sqlx-mysql")]
    pub async fn begin_mysql_root(
        &mut self,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<(), DbErr> {
        self.ensure_session_usable()?;
        if self.backend != DbBackend::MySql {
            return Err(conn_err("保留会话根事务仅支持 MySQL"));
        }
        if self.open || Arc::strong_count(&self.conn) != 1 {
            return Err(conn_err(
                "开启保留会话根事务需要独占且已结束的持有者，拒绝嵌套或共享事务",
            ));
        }
        #[cfg(not(feature = "sync"))]
        let mut guard = self.conn.lock().await;
        #[cfg(feature = "sync")]
        let mut guard = self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;
        match &mut *guard {
            InnerConnection::MySql(connection) => {
                if <sqlx::MySql as sqlx::Database>::TransactionManager::get_transaction_depth(
                    connection,
                ) != 0
                {
                    return Err(conn_err("begin_mysql_root 仅接受事务深度零"));
                }
                connection.close_on_drop();
                self.session_invalid = true;
                crate::driver::sqlx_mysql::set_transaction_config(
                    connection,
                    isolation_level,
                    access_mode,
                )
                .await?;
                <sqlx::MySql as sqlx::Database>::TransactionManager::begin(connection, None)
                    .await
                    .map_err(sqlx_error_to_query_err)?;
                self.open = true;
                self.session_invalid = false;
                Ok(())
            }
            #[cfg(any(
                feature = "sqlx-postgres",
                feature = "sqlx-sqlite",
                feature = "rusqlite",
                feature = "mock",
                feature = "proxy"
            ))]
            _ => Err(conn_err("保留会话根事务仅支持 MySQL")),
        }
    }
    /// Commit a transaction
    #[instrument(level = "trace")]
    #[allow(unreachable_code, unused_mut)]
    pub async fn commit(mut self) -> Result<(), DbErr> {
        self.ensure_session_usable()?;
        let result: Result<(), DbErr> = super::tracing_spans::with_db_span!(
            "sea_orm.commit",
            self.backend,
            "COMMIT",
            record_stmt = false,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(c) => {
                        <sqlx::MySql as sqlx::Database>::TransactionManager::commit(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(c) => {
                        <sqlx::Postgres as sqlx::Database>::TransactionManager::commit(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(c) => {
                        <sqlx::Sqlite as sqlx::Database>::TransactionManager::commit(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(c) => c.commit(),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(c) => {
                        c.commit();
                        Ok(())
                    }
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(c) => {
                        c.commit().await;
                        Ok(())
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        );

        result?;
        self.open = false; // read by start_rollback
        Ok(())
    }

    /// Rolls back a transaction explicitly
    #[instrument(level = "trace")]
    #[allow(unreachable_code, unused_mut)]
    pub async fn rollback(mut self) -> Result<(), DbErr> {
        self.ensure_session_usable()?;
        let result: Result<(), DbErr> = super::tracing_spans::with_db_span!(
            "sea_orm.rollback",
            self.backend,
            "ROLLBACK",
            record_stmt = false,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(c) => {
                        <sqlx::MySql as sqlx::Database>::TransactionManager::rollback(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(c) => {
                        <sqlx::Postgres as sqlx::Database>::TransactionManager::rollback(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(c) => {
                        <sqlx::Sqlite as sqlx::Database>::TransactionManager::rollback(c)
                            .await
                            .map_err(sqlx_error_to_query_err)
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(c) => c.rollback(),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(c) => {
                        c.rollback();
                        Ok(())
                    }
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(c) => {
                        c.rollback().await;
                        Ok(())
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        );

        result?;
        self.open = false; // read by start_rollback
        Ok(())
    }

    // the rollback is queued and will be performed on next async operation, like returning the connection to the pool
    #[instrument(level = "trace")]
    fn start_rollback(&mut self) -> Result<(), DbErr> {
        if self.open {
            if let Some(mut conn) = self.conn.try_lock() {
                match &mut *conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(c) => {
                        <sqlx::MySql as sqlx::Database>::TransactionManager::start_rollback(c);
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(c) => {
                        <sqlx::Postgres as sqlx::Database>::TransactionManager::start_rollback(c);
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(c) => {
                        <sqlx::Sqlite as sqlx::Database>::TransactionManager::start_rollback(c);
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(c) => {
                        c.start_rollback()?;
                    }
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(c) => {
                        c.rollback();
                    }
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(c) => {
                        c.start_rollback();
                    }
                    #[allow(unreachable_patterns)]
                    _ => return Err(conn_err("Disconnected")),
                }
            } else {
                //this should never happen
                return Err(conn_err("Dropping a locked Transaction"));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl TransactionSession for DatabaseTransaction {
    async fn commit(self) -> Result<(), DbErr> {
        self.commit().await
    }

    async fn rollback(self) -> Result<(), DbErr> {
        self.rollback().await
    }
}

impl Drop for DatabaseTransaction {
    fn drop(&mut self) {
        self.start_rollback().expect("Fail to rollback transaction");
    }
}

#[async_trait::async_trait]
impl ConnectionTrait for DatabaseTransaction {
    fn get_database_backend(&self) -> DbBackend {
        // this way we don't need to lock just to know the backend
        self.backend
    }

    #[instrument(level = "trace", skip(stmt))]
    #[allow(unused_variables)]
    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.ensure_session_usable()?;
        debug_print!("{}", stmt);

        super::tracing_spans::with_db_span!(
            "sea_orm.execute",
            self.backend,
            stmt.sql.as_str(),
            record_stmt = self.record_stmt_in_spans,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(conn) => {
                        let query = crate::driver::sqlx_mysql::sqlx_query(&stmt);
                        let conn: &mut sqlx::MySqlConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query.execute(conn).await.map(Into::into)
                        })
                        .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(conn) => {
                        let query = crate::driver::sqlx_postgres::sqlx_query(&stmt);
                        let conn: &mut sqlx::PgConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query.execute(conn).await.map(Into::into)
                        })
                        .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(conn) => {
                        let query = crate::driver::sqlx_sqlite::sqlx_query(&stmt);
                        let conn: &mut sqlx::SqliteConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query.execute(conn).await.map(Into::into)
                        })
                        .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(conn) => conn.execute(stmt, &self.metric_callback),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(conn) => conn.execute(stmt),
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(conn) => conn.execute(stmt).await,
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        )
    }

    #[instrument(level = "trace", skip(sql))]
    #[allow(unused_variables)]
    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.ensure_session_usable()?;
        debug_print!("{}", sql);

        super::tracing_spans::with_db_span!(
            "sea_orm.execute_unprepared",
            self.backend,
            sql,
            record_stmt = false,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(conn) => {
                        let conn: &mut sqlx::MySqlConnection = &mut *conn;
                        sqlx::Executor::execute(conn, sqlx::AssertSqlSafe(sql.to_owned()))
                            .await
                            .map(Into::into)
                            .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(conn) => {
                        let conn: &mut sqlx::PgConnection = &mut *conn;
                        sqlx::Executor::execute(conn, sqlx::AssertSqlSafe(sql.to_owned()))
                            .await
                            .map(Into::into)
                            .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(conn) => {
                        let conn: &mut sqlx::SqliteConnection = &mut *conn;
                        sqlx::Executor::execute(conn, sqlx::AssertSqlSafe(sql.to_owned()))
                            .await
                            .map(Into::into)
                            .map_err(sqlx_error_to_exec_err)
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(conn) => conn.execute_unprepared(sql),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(conn) => {
                        let db_backend = conn.get_database_backend();
                        let stmt = Statement::from_string(db_backend, sql);
                        conn.execute(stmt)
                    }
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(conn) => {
                        let db_backend = conn.get_database_backend();
                        let stmt = Statement::from_string(db_backend, sql);
                        conn.execute(stmt).await
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        )
    }

    #[instrument(level = "trace", skip(stmt))]
    #[allow(unused_variables)]
    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.ensure_session_usable()?;
        debug_print!("{}", stmt);

        super::tracing_spans::with_db_span!(
            "sea_orm.query_one",
            self.backend,
            stmt.sql.as_str(),
            record_stmt = self.record_stmt_in_spans,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(conn) => {
                        let query = crate::driver::sqlx_mysql::sqlx_query(&stmt);
                        let conn: &mut sqlx::MySqlConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            crate::sqlx_map_err_ignore_not_found(
                                query.fetch_one(conn).await.map(|row| Some(row.into())),
                            )
                        })
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(conn) => {
                        let query = crate::driver::sqlx_postgres::sqlx_query(&stmt);
                        let conn: &mut sqlx::PgConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            crate::sqlx_map_err_ignore_not_found(
                                query.fetch_one(conn).await.map(|row| Some(row.into())),
                            )
                        })
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(conn) => {
                        let query = crate::driver::sqlx_sqlite::sqlx_query(&stmt);
                        let conn: &mut sqlx::SqliteConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            crate::sqlx_map_err_ignore_not_found(
                                query.fetch_one(conn).await.map(|row| Some(row.into())),
                            )
                        })
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(conn) => conn.query_one(stmt, &self.metric_callback),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(conn) => conn.query_one(stmt),
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(conn) => conn.query_one(stmt).await,
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        )
    }

    #[instrument(level = "trace", skip(stmt))]
    #[allow(unused_variables)]
    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.ensure_session_usable()?;
        debug_print!("{}", stmt);

        super::tracing_spans::with_db_span!(
            "sea_orm.query_all",
            self.backend,
            stmt.sql.as_str(),
            record_stmt = self.record_stmt_in_spans,
            async {
                #[cfg(not(feature = "sync"))]
                let conn = &mut *self.conn.lock().await;
                #[cfg(feature = "sync")]
                let conn = &mut *self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;

                match conn {
                    #[cfg(feature = "sqlx-mysql")]
                    InnerConnection::MySql(conn) => {
                        let query = crate::driver::sqlx_mysql::sqlx_query(&stmt);
                        let conn: &mut sqlx::MySqlConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query
                                .fetch_all(conn)
                                .await
                                .map(|rows| rows.into_iter().map(|r| r.into()).collect())
                                .map_err(sqlx_error_to_query_err)
                        })
                    }
                    #[cfg(feature = "sqlx-postgres")]
                    InnerConnection::Postgres(conn) => {
                        let query = crate::driver::sqlx_postgres::sqlx_query(&stmt);
                        let conn: &mut sqlx::PgConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query
                                .fetch_all(conn)
                                .await
                                .map(|rows| rows.into_iter().map(|r| r.into()).collect())
                                .map_err(sqlx_error_to_query_err)
                        })
                    }
                    #[cfg(feature = "sqlx-sqlite")]
                    InnerConnection::Sqlite(conn) => {
                        let query = crate::driver::sqlx_sqlite::sqlx_query(&stmt);
                        let conn: &mut sqlx::SqliteConnection = &mut *conn;
                        crate::metric::metric!(self.metric_callback, &stmt, {
                            query
                                .fetch_all(conn)
                                .await
                                .map(|rows| rows.into_iter().map(|r| r.into()).collect())
                                .map_err(sqlx_error_to_query_err)
                        })
                    }
                    #[cfg(feature = "rusqlite")]
                    InnerConnection::Rusqlite(conn) => conn.query_all(stmt, &self.metric_callback),
                    #[cfg(feature = "mock")]
                    InnerConnection::Mock(conn) => conn.query_all(stmt),
                    #[cfg(feature = "proxy")]
                    InnerConnection::Proxy(conn) => conn.query_all(stmt).await,
                    #[allow(unreachable_patterns)]
                    _ => Err(conn_err("Disconnected")),
                }
            }
        )
    }
}

#[cfg(feature = "stream")]
impl StreamTrait for DatabaseTransaction {
    type Stream<'a> = TransactionStream<'a>;

    fn get_database_backend(&self) -> DbBackend {
        self.backend
    }

    #[instrument(level = "trace", skip(stmt))]
    fn stream_raw<'a>(
        &'a self,
        stmt: Statement,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Stream<'a>, DbErr>> + 'a + Send>> {
        Box::pin(async move {
            self.ensure_session_usable()?;
            #[cfg(not(feature = "sync"))]
            let conn = self.conn.lock().await;
            #[cfg(feature = "sync")]
            let conn = self.conn.lock().map_err(|_| DbErr::MutexPoisonError)?;
            Ok(crate::TransactionStream::build(
                conn,
                stmt,
                self.metric_callback.clone(),
            ))
        })
    }
}

#[async_trait::async_trait]
impl TransactionTrait for DatabaseTransaction {
    type Transaction = DatabaseTransaction;

    #[instrument(level = "trace")]
    async fn begin(&self) -> Result<DatabaseTransaction, DbErr> {
        self.ensure_session_usable()?;
        DatabaseTransaction::begin(
            Arc::clone(&self.conn),
            self.backend,
            self.metric_callback.clone(),
            self.record_stmt_in_spans,
            None,
            None,
            None,
        )
        .await
    }

    #[instrument(level = "trace")]
    async fn begin_with_config(
        &self,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<DatabaseTransaction, DbErr> {
        self.ensure_session_usable()?;
        DatabaseTransaction::begin(
            Arc::clone(&self.conn),
            self.backend,
            self.metric_callback.clone(),
            self.record_stmt_in_spans,
            isolation_level,
            access_mode,
            None,
        )
        .await
    }

    #[instrument(level = "trace")]
    async fn begin_with_options(
        &self,
        options: TransactionOptions,
    ) -> Result<DatabaseTransaction, DbErr> {
        self.ensure_session_usable()?;
        DatabaseTransaction::begin(
            Arc::clone(&self.conn),
            self.backend,
            self.metric_callback.clone(),
            self.record_stmt_in_spans,
            options.isolation_level,
            options.access_mode,
            options.sqlite_transaction_mode,
        )
        .await
    }

    /// Execute the async function inside a transaction.
    /// If the function returns an error, the transaction will be rolled back.
    /// Otherwise, the transaction will be committed.
    #[instrument(level = "trace", skip(_callback))]
    async fn transaction<F, T, E>(&self, _callback: F) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(
                &'c DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let transaction = self.begin().await.map_err(TransactionError::Connection)?;
        transaction.run(_callback).await
    }

    /// Execute the async function inside a transaction.
    /// If the function returns an error, the transaction will be rolled back.
    /// Otherwise, the transaction will be committed.
    #[instrument(level = "trace", skip(_callback))]
    async fn transaction_with_config<F, T, E>(
        &self,
        _callback: F,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(
                &'c DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        let transaction = self
            .begin_with_config(isolation_level, access_mode)
            .await
            .map_err(TransactionError::Connection)?;
        transaction.run(_callback).await
    }
}

/// Error returned by [`TransactionTrait::transaction`](crate::TransactionTrait::transaction):
/// either the database itself failed, or the user closure returned an `Err`
/// (causing a rollback).
#[derive(Debug)]
pub enum TransactionError<E> {
    /// Failure issuing `BEGIN`, `COMMIT`, or `ROLLBACK`.
    Connection(DbErr),
    /// The closure returned an `Err` (the transaction has been rolled back).
    Transaction(E),
}

impl<E> std::fmt::Display for TransactionError<E>
where
    E: std::fmt::Display + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionError::Connection(e) => std::fmt::Display::fmt(e, f),
            TransactionError::Transaction(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl<E> std::error::Error for TransactionError<E> where E: std::fmt::Display + std::fmt::Debug {}

impl<E> From<DbErr> for TransactionError<E>
where
    E: std::fmt::Display + std::fmt::Debug,
{
    fn from(e: DbErr) -> Self {
        Self::Connection(e)
    }
}

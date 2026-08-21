use std::ops::Deref;

use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, ExecResult, QueryResult, Statement,
};

/// 控制库事务的本地包装，允许数据库适配器实现应用事务端口。
#[doc(hidden)]
pub struct DatabasePortTransaction(DatabaseTransaction);

impl From<DatabaseTransaction> for DatabasePortTransaction {
    fn from(transaction: DatabaseTransaction) -> Self {
        Self(transaction)
    }
}

impl Deref for DatabasePortTransaction {
    type Target = DatabaseTransaction;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait::async_trait]
impl ConnectionTrait for DatabasePortTransaction {
    fn get_database_backend(&self) -> DbBackend {
        self.0.get_database_backend()
    }

    async fn execute_raw(&self, statement: Statement) -> Result<ExecResult, DbErr> {
        self.0.execute_raw(statement).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.0.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, statement: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.0.query_one_raw(statement).await
    }

    async fn query_all_raw(&self, statement: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.0.query_all_raw(statement).await
    }

    fn is_mock_connection(&self) -> bool {
        self.0.is_mock_connection()
    }
}

impl DatabasePortTransaction {
    pub(super) fn into_inner(self) -> DatabaseTransaction {
        self.0
    }

    #[doc(hidden)]
    pub async fn commit(self) -> Result<(), DbErr> {
        self.0.commit().await
    }

    #[doc(hidden)]
    pub async fn rollback(self) -> Result<(), DbErr> {
        self.0.rollback().await
    }

    #[doc(hidden)]
    pub async fn commit_audited(self) -> ryframe_kernel::AppResult<()> {
        super::audit_persistence::commit_current_audit(self.0).await
    }
}

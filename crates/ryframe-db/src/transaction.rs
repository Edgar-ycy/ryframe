use std::future::Future;

use ryframe_common::{AppError, AppResult};
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};

/// 闭包式事务管理器
///
/// 使用方式：
///
/// ```
/// use ryframe_db::transaction::Transaction;
/// use ryframe_common::AppResult;
/// use sea_orm::DatabaseTransaction;
/// use std::pin::Pin;
/// use std::future::Future;
///
/// // Transaction::run 接受一个与下方签名兼容的闭包：
/// fn create_user(tx: &DatabaseTransaction) -> Pin<Box<dyn Future<Output = AppResult<()>> + '_>> {
///     Box::pin(async move {
///         let _ = tx;
///         // 所有使用 tx 的数据库操作在同一事务中执行
///         // user_repo.insert(tx, user).await?;
///         Ok(())
///     })
/// }
///
/// // 实际调用时需要 DatabaseConnection：
/// // Transaction::run(db, |tx| Box::pin(async { Ok(()) })).await?;
/// ```
pub struct Transaction;

impl Transaction {
    /// 在事务中执行闭包，自动 commit 或 rollback
    pub async fn run<F, Fut>(db: &DatabaseConnection, f: F) -> AppResult<()>
    where
        F: FnOnce(&DatabaseTransaction) -> Fut,
        Fut: Future<Output = AppResult<()>>,
    {
        let tx = db
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("开启事务失败: {}", e)))?;

        match f(&tx).await {
            Ok(()) => tx
                .commit()
                .await
                .map_err(|e| AppError::Database(format!("提交事务失败: {}", e))),
            Err(err) => {
                // rollback 失败不覆盖原始错误
                if let Err(e) = tx.rollback().await {
                    log::error!("回滚事务失败: {}", e);
                }
                Err(err)
            }
        }
    }
}

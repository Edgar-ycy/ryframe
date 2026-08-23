use async_trait::async_trait;
use ryframe_kernel::AppResult;

/// 提交事务时采用的审计策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransactionAuditMode {
    /// 将当前请求绑定的审计事件与业务数据一同提交。
    #[default]
    CurrentRequest,
    /// 明确跳过请求审计，供没有请求上下文的内部事务使用。
    Skip,
}

/// 由应用用例显式控制生命周期的持久化事务。
#[async_trait]
pub trait PersistenceTransaction: Send + Sync {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()>;

    async fn rollback(self: Box<Self>) -> AppResult<()>;
}

/// 根据业务执行结果完成事务，同时保留最有价值的业务错误。
pub async fn complete_transaction<T, R>(
    transaction: Box<T>,
    operation: AppResult<R>,
    audit_mode: TransactionAuditMode,
) -> AppResult<R>
where
    T: PersistenceTransaction + ?Sized,
{
    match operation {
        Ok(value) => {
            transaction.commit(audit_mode).await?;
            Ok(value)
        }
        Err(operation_error) => {
            if let Err(rollback_error) = transaction.rollback().await {
                tracing::error!(
                    error = %rollback_error,
                    "业务执行失败后的事务回滚失败"
                );
            }
            Err(operation_error)
        }
    }
}

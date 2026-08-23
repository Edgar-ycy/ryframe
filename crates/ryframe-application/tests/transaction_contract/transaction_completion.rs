use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use ryframe_application::{PersistenceTransaction, TransactionAuditMode, complete_transaction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Call {
    Commit(TransactionAuditMode),
    Rollback,
}

struct FakeTransaction {
    calls: Arc<Mutex<Vec<Call>>>,
    rollback_fails: bool,
}

#[async_trait]
impl PersistenceTransaction for FakeTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        self.calls
            .lock()
            .expect("调用记录锁应可用")
            .push(Call::Commit(audit_mode));
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.calls
            .lock()
            .expect("调用记录锁应可用")
            .push(Call::Rollback);
        if self.rollback_fails {
            Err(AppError::Internal("模拟回滚失败".into()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn success_forwards_the_requested_audit_mode() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let result = complete_transaction(
        Box::new(FakeTransaction {
            calls: Arc::clone(&calls),
            rollback_fails: false,
        }),
        Ok(7),
        TransactionAuditMode::Skip,
    )
    .await
    .expect("事务应提交成功");

    assert_eq!(result, 7);
    assert_eq!(
        *calls.lock().expect("调用记录锁应可用"),
        [Call::Commit(TransactionAuditMode::Skip)]
    );
}

#[tokio::test]
async fn failure_rolls_back_without_hiding_the_operation_error() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let error = complete_transaction(
        Box::new(FakeTransaction {
            calls: Arc::clone(&calls),
            rollback_fails: true,
        }),
        Err::<(), _>(AppError::Conflict("业务冲突".into())),
        TransactionAuditMode::CurrentRequest,
    )
    .await
    .expect_err("业务操作失败时应返回错误");

    assert!(matches!(error, AppError::Conflict(message) if message == "业务冲突"));
    assert_eq!(*calls.lock().expect("调用记录锁应可用"), [Call::Rollback]);
}

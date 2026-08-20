use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type PersistenceFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// 由应用用例控制提交时机的控制库事务。
pub trait ControlTransaction: Send {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

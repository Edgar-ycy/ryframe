//! 与浏览器共享同一版本账本的登录预算；失败尝试仍然占用预算。

#[path = "login_budget/files.rs"]
mod files;
#[path = "login_budget/locking.rs"]
mod locking;
#[path = "login_budget/model.rs"]
mod model;
#[path = "login_budget/paths.rs"]
mod paths;

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) use model::{Binding, Reservation, ReserveOutcome};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BudgetError {
    Scope,
    Capacity,
    Window,
    Identity,
    Clock,
    Ledger,
    Bucket,
    BucketsFull,
    Reservation,
    Path,
    File,
    Size,
    Io,
    LockBusy,
    LockOwner,
    PublicationUnknown,
}

impl BudgetError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Scope => "登录预算必须绑定明确的隔离 scope",
            Self::Capacity => "登录预算容量必须来自有效后端配置",
            Self::Window => "登录预算窗口必须来自有效后端配置",
            Self::Identity => "登录预算缺少身份或固定测试客户地址",
            Self::Clock => "登录预算时间必须是非负安全整数，且不能回退",
            Self::Ledger => "登录预算账本损坏、时间回退或 scope/限流配置不一致",
            Self::Bucket => "登录预算账本包含损坏条目或未来时间",
            Self::BucketsFull => "登录预算活跃条目超过上限，拒绝丢弃有效预算",
            Self::Reservation => "登录响应超出已预约窗口，不能确认当前预算",
            Self::Path => "登录预算必须使用无链接的绝对文件路径",
            Self::File => "登录预算必须是有限大小的普通账本文件",
            Self::Size => "登录预算账本超过大小上限",
            Self::Io => "登录预算文件读写失败，已有账本保持待核验",
            Self::LockBusy => "登录预算文件锁持续占用，请核验对应测试进程；不会强抢锁",
            Self::LockOwner => "登录预算锁身份或所有者发生变化，拒绝删除当前锁",
            Self::PublicationUnknown => {
                "登录预算发布或解锁结果不确定，必须核验账本与锁；不得自动重放"
            }
        }
    }
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for BudgetError {}

pub(crate) struct LedgerFile {
    path: PathBuf,
    binding: Binding,
    uncertain: AtomicBool,
}

impl LedgerFile {
    /// 构造时只校验来源，不创建目录、锁或账本。
    pub(crate) fn new(path: PathBuf, binding: Binding) -> Result<Self, BudgetError> {
        paths::validate(&path)?;
        Binding::new(&binding.scope, binding.capacity, binding.window_ms)?;
        Ok(Self {
            path,
            binding,
            uncertain: AtomicBool::new(false),
        })
    }

    pub(crate) fn try_reserve(
        &self,
        tenant_id: &str,
        username: &str,
        address: &str,
        now_ms: u64,
    ) -> Result<ReserveOutcome, BudgetError> {
        let keys = model::login_keys(tenant_id, username, address)?;
        self.update(now_ms, |ledger| {
            ledger.reserve(&self.binding, &keys, now_ms)
        })
    }

    pub(crate) fn complete(
        &self,
        reservation: &[Reservation],
        now_ms: u64,
    ) -> Result<(), BudgetError> {
        self.update(now_ms, |ledger| {
            ledger.complete(&self.binding, reservation, now_ms)
        })
    }

    fn update<T>(
        &self,
        now_ms: u64,
        update: impl FnOnce(&mut model::Ledger) -> Result<T, BudgetError>,
    ) -> Result<T, BudgetError> {
        model::validate_time(now_ms)?;
        if self.uncertain.load(Ordering::Acquire) {
            return Err(BudgetError::PublicationUnknown);
        }
        let parents = paths::Parents::prepare(&self.path)?;
        let mut lock = locking::OwnedLock::acquire(&self.path, &parents)?;
        let result = (|| {
            if self.uncertain.load(Ordering::Acquire) {
                return Err(BudgetError::PublicationUnknown);
            }
            parents.verify()?;
            lock.verify()?;
            let mut current = files::read(&self.path, &self.binding, now_ms)?;
            let result = update(&mut current.ledger)?;
            files::write(&self.path, &current, &parents, &lock)?;
            Ok(result)
        })();
        let released = lock.release();
        if matches!(result, Err(BudgetError::PublicationUnknown)) || released.is_err() {
            self.uncertain.store(true, Ordering::Release);
        }
        match (result, released) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (_, Err(_)) => Err(BudgetError::PublicationUnknown),
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/login_budget/controller.rs"]
mod tests;

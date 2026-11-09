//! 启动相位使用各端本地单调时钟；重试和成功返回都不延长原相位。
use super::diagnostic::{Category, Outcome, failure};
use crate::Result;
use std::time::{Duration, Instant};

pub(super) const ACCEPTANCE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
pub(super) struct Deadline(Instant);

impl Deadline {
    pub(super) fn new(start: Instant, budget: Duration) -> Self {
        Self(start + budget)
    }

    pub(super) fn check(self, now: Instant, category: Category) -> Result<()> {
        if now >= self.0 {
            return Err(failure(category, Outcome::Unknown));
        }
        Ok(())
    }

    pub(super) fn accepting(self, now: Instant, category: Category) -> Result<Self> {
        self.check(now, category)?;
        Ok(Self::new(now, ACCEPTANCE))
    }
}

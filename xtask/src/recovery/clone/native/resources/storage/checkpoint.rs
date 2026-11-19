//! 按阶段报告存储启动失败；公开收据不包含底层错误原文。
use crate::Result;

#[derive(Clone, Copy, Debug)]
pub(super) enum Point {
    Controller,
    Lock,
    Registration,
    Source,
    Producers,
    Guard,
    Launch,
}

impl Point {
    fn name(self) -> &'static str {
        match self {
            Self::Controller => "StorageController",
            Self::Lock => "StorageLock",
            Self::Registration => "StorageRegistration",
            Self::Source => "StorageSourcePredecessor",
            Self::Producers => "StorageProducerStop",
            Self::Guard => "StorageRestartGuard",
            Self::Launch => "StorageProductLaunch",
        }
    }

    pub(super) fn check<T>(self, result: Result<T>) -> Result<T> {
        result.map_err(|error| {
            if error.is::<Failure>() {
                error
            } else {
                Box::new(Failure(self, error))
            }
        })
    }
}

#[derive(Debug)]
struct Failure(Point, Box<dyn std::error::Error>);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "存储阶段检查失败（{}）", self.0.name())
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.1.as_ref())
    }
}

pub(crate) fn diagnostic(
    error: &(dyn std::error::Error + 'static),
) -> Option<(&'static str, String)> {
    let failure = error.downcast_ref::<Failure>()?;
    Some((failure.0.name(), failure.to_string()))
}

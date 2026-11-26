//! 标记恢复的前置失败仅发布固定位置，不公开底层错误内容。
use std::{error::Error, fmt};

use crate::Result;

#[derive(Clone, Copy, Debug)]
pub(in crate::recovery::clone::native) enum MarkerCheckpoint {
    RunBinding,
    ApiIncidentHistory,
    ApiIncidentRuntime,
    ApiIncidentObservation,
    ApiIncidentFiles,
    PreparationHistory,
    StorageLock,
    TargetOwner,
    CopyLedgerLock,
    TargetBinding,
    FixtureSuccessor,
    MarkerModel,
    CopyProof,
    SourceGeneration,
    LedgerPrefix,
    FinalGuards,
    ReceiptOpen,
}

impl MarkerCheckpoint {
    fn label(self) -> &'static str {
        match self {
            Self::RunBinding => "MarkerRecoveryRunBinding",
            Self::ApiIncidentHistory => "MarkerRecoveryApiIncidentHistory",
            Self::ApiIncidentRuntime => "MarkerRecoveryApiIncidentRuntime",
            Self::ApiIncidentObservation => "MarkerRecoveryApiIncidentObservation",
            Self::ApiIncidentFiles => "MarkerRecoveryApiIncidentFiles",
            Self::PreparationHistory => "MarkerRecoveryPreparationHistory",
            Self::StorageLock => "MarkerRecoveryStorageLock",
            Self::TargetOwner => "MarkerRecoveryTargetOwner",
            Self::CopyLedgerLock => "MarkerRecoveryCopyLedgerLock",
            Self::TargetBinding => "MarkerRecoveryTargetBinding",
            Self::FixtureSuccessor => "MarkerRecoveryFixtureSuccessor",
            Self::MarkerModel => "MarkerRecoveryMarkerModel",
            Self::CopyProof => "MarkerRecoveryCopyProof",
            Self::SourceGeneration => "MarkerRecoverySourceGeneration",
            Self::LedgerPrefix => "MarkerRecoveryLedgerPrefix",
            Self::FinalGuards => "MarkerRecoveryFinalGuards",
            Self::ReceiptOpen => "MarkerRecoveryReceiptOpen",
        }
    }

    pub(in crate::recovery::clone::native) fn check<T>(self, result: Result<T>) -> Result<T> {
        result.map_err(|error| {
            if error.is::<Failure>() {
                error
            } else {
                Box::new(Failure(self))
            }
        })
    }
}

#[derive(Debug)]
struct Failure(MarkerCheckpoint);

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.label())
    }
}

impl Error for Failure {}

pub(super) fn category(error: &(dyn Error + 'static)) -> Option<&'static str> {
    error
        .downcast_ref::<Failure>()
        .map(|failure| failure.0.label())
}

#[cfg(test)]
#[test]
fn marker_checkpoint_reports_fixed_category_without_underlying_payload() {
    let payload = "private-test-marker-with-payload";
    let result = MarkerCheckpoint::LedgerPrefix.check::<()>(Err(payload.into()));
    let failure = MarkerCheckpoint::CopyProof.check(result).unwrap_err();
    assert_eq!(category(&*failure), Some("MarkerRecoveryLedgerPrefix"));
    assert_eq!(super::failure_type(&*failure), "MarkerRecoveryLedgerPrefix");
    assert_eq!(
        super::safe_diagnostic(&*failure).as_deref(),
        Some("Redis 标记恢复前置检查失败（MarkerRecoveryLedgerPrefix）")
    );
    assert_eq!(failure.to_string(), "MarkerRecoveryLedgerPrefix");
    assert!(!format!("{failure:?}").contains(payload));
    assert!(failure.source().is_none());
}

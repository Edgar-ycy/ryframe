use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait};

use crate::{DbResultExt, entities::background_job};

use super::{BackgroundJobRepository, JobFailureDisposition, truncate_error};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkedJobDisposition {
    Retried,
    Dead,
    ManuallyRetried,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkedBusinessTerminal {
    Succeeded,
    Cancelled,
    Expired,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkedJobSyncResult {
    NotLinked,
    Transitioned,
    Terminal(LinkedBusinessTerminal),
    Conflict,
}

impl LinkedJobSyncResult {
    fn terminal_disposition(self) -> AppResult<Option<JobFailureDisposition>> {
        match self {
            Self::NotLinked | Self::Transitioned => Ok(None),
            Self::Terminal(LinkedBusinessTerminal::Failed) => Ok(Some(JobFailureDisposition::Dead)),
            Self::Terminal(
                LinkedBusinessTerminal::Succeeded
                | LinkedBusinessTerminal::Cancelled
                | LinkedBusinessTerminal::Expired,
            ) => Ok(Some(JobFailureDisposition::Completed)),
            Self::Conflict => Err(AppError::Conflict(
                "关联业务任务状态已变化，拒绝覆盖权威状态".into(),
            )),
        }
    }
}

impl BackgroundJobRepository {
    pub(super) async fn finish_from_linked_terminal<C>(
        db: &C,
        job: &background_job::Model,
        sync_result: LinkedJobSyncResult,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<Option<JobFailureDisposition>>
    where
        C: ConnectionTrait,
    {
        let Some(disposition) = sync_result.terminal_disposition()? else {
            return Ok(None);
        };
        let mut active: background_job::ActiveModel = job.clone().into();
        active.status = Set(if matches!(disposition, JobFailureDisposition::Completed) {
            background_job::Model::STATUS_SUCCEEDED
        } else {
            background_job::Model::STATUS_DEAD
        }
        .to_owned());
        active.available_at = Set(now);
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        if matches!(disposition, JobFailureDisposition::Dead) {
            active.last_error = Set(error_message.map(truncate_error).or(job.last_error.clone()));
        }
        active.updated_at = Set(now);
        active.completed_at = Set(Some(now));
        active.update(db).await.db()?;
        Ok(Some(disposition))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_terminals_map_to_background_terminal_states() {
        for (terminal, expected) in [
            (
                LinkedBusinessTerminal::Succeeded,
                JobFailureDisposition::Completed,
            ),
            (
                LinkedBusinessTerminal::Cancelled,
                JobFailureDisposition::Completed,
            ),
            (
                LinkedBusinessTerminal::Expired,
                JobFailureDisposition::Completed,
            ),
            (LinkedBusinessTerminal::Failed, JobFailureDisposition::Dead),
        ] {
            assert_eq!(
                LinkedJobSyncResult::Terminal(terminal)
                    .terminal_disposition()
                    .expect("终态映射不应失败"),
                Some(expected)
            );
        }
    }

    #[test]
    fn conflicting_linked_state_fails_closed() {
        assert!(
            LinkedJobSyncResult::Conflict
                .terminal_disposition()
                .is_err()
        );
    }
}

use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::{backup_content_hash, has_storage_timestamp_precision};

/// 恢复演练只绑定显式登记的隔离目标。连接凭据由进程外的配置提供。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestorePlan {
    pub id: String,
    pub backup_id: String,
    pub scope_id: String,
    pub fault_at: DateTime<Utc>,
    pub databases: Vec<RestoreDatabase>,
    pub object_endpoint: String,
    pub object_prefix: String,
    pub api_ready_url: String,
    pub worker_ready_url: String,
    pub frontend_sha: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreDatabase {
    pub source_key: String,
    pub target_key: String,
    pub server_uuid: String,
    pub database: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    Running,
    DataVerified,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRecord {
    pub plan: RestorePlan,
    pub plan_hash: String,
    pub status: RestoreStatus,
    pub started_at: DateTime<Utc>,
    pub data_verified_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub recovered_at: DateTime<Utc>,
    pub failure: Option<String>,
}

/// 校验首次持久化的恢复演练记录。
pub fn validate_restore_creation(record: &RestoreRecord) -> AppResult<()> {
    validate_restore_record(record)?;
    if record.status != RestoreStatus::Running {
        return Err(AppError::Validation("恢复演练初始状态必须为运行中".into()));
    }
    Ok(())
}

/// 校验恢复演练的一次原子状态推进。
pub fn validate_restore_advance(expected: &RestoreRecord, next: &RestoreRecord) -> AppResult<()> {
    validate_restore_record(expected)?;
    if expected.plan != next.plan
        || expected.plan_hash != next.plan_hash
        || expected.started_at != next.started_at
        || expected.recovered_at != next.recovered_at
        || expected.data_verified_at.is_some() && expected.data_verified_at != next.data_verified_at
    {
        return Err(AppError::Conflict("恢复演练不可变信息已变化".into()));
    }
    if !matches!(
        (expected.status, next.status),
        (RestoreStatus::Running, RestoreStatus::DataVerified)
            | (RestoreStatus::Running, RestoreStatus::Failed)
            | (RestoreStatus::DataVerified, RestoreStatus::Succeeded)
            | (RestoreStatus::DataVerified, RestoreStatus::Failed)
    ) {
        return Err(AppError::Conflict("恢复演练状态转换无效".into()));
    }
    validate_restore_record(next)?;
    if expected.status == RestoreStatus::Running
        && next.status == RestoreStatus::Failed
        && next.data_verified_at.is_some()
    {
        return Err(AppError::Validation(
            "未完成数据验证的失败演练不能登记验证时间".into(),
        ));
    }
    Ok(())
}

/// 校验任意已持久化恢复记录的摘要、状态与时间投影。
pub fn validate_restore_record(record: &RestoreRecord) -> AppResult<()> {
    let timestamps = [
        Some(record.plan.fault_at),
        Some(record.started_at),
        record.data_verified_at,
        record.completed_at,
        Some(record.recovered_at),
    ];
    if timestamps
        .into_iter()
        .flatten()
        .any(|timestamp| !has_storage_timestamp_precision(timestamp))
        || backup_content_hash(&record.plan)? != record.plan_hash
        || record.recovered_at > record.started_at
    {
        return Err(AppError::Validation(
            "恢复演练计划摘要、时间精度或恢复点时间无效".into(),
        ));
    }
    let verified = record.data_verified_at.is_some_and(|verified| {
        verified >= record.started_at && verified - record.started_at <= Duration::hours(1)
    });
    let verified_or_absent = record.data_verified_at.is_none() || verified;
    let completed_after = |minimum| {
        record
            .completed_at
            .is_some_and(|completed| completed >= minimum)
    };
    let failure = record
        .failure
        .as_deref()
        .is_some_and(|message| !message.trim().is_empty() && message.chars().count() <= 1000);
    let valid = match record.status {
        RestoreStatus::Running => {
            record.data_verified_at.is_none()
                && record.completed_at.is_none()
                && record.failure.is_none()
        }
        RestoreStatus::DataVerified => {
            verified && record.completed_at.is_none() && record.failure.is_none()
        }
        RestoreStatus::Succeeded => {
            verified
                && record.data_verified_at.is_some_and(&completed_after)
                && record
                    .completed_at
                    .is_some_and(|completed| completed - record.started_at <= Duration::hours(1))
                && record.failure.is_none()
        }
        RestoreStatus::Failed => {
            verified_or_absent
                && completed_after(record.data_verified_at.unwrap_or(record.started_at))
                && failure
        }
    };
    if !valid {
        return Err(AppError::Validation(
            "恢复演练状态、时间或失败信息无效".into(),
        ));
    }
    Ok(())
}

/// 由真实服务浏览器验收入口写出的结果；必须绑定当前演练与精确源码。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBusinessProof {
    pub restore_id: String,
    pub plan_hash: String,
    pub backup_source_sha: String,
    pub backend_product_sha: String,
    pub backend_execution_sha: String,
    pub backend_adapter_contract: Option<String>,
    pub frontend_sha: String,
    pub runner_sha: String,
    pub verifier_sha: String,
    pub scope_id: String,
    pub frontend_url: String,
    pub runtime_receipt_sha256: String,
    pub tests_receipt_sha256: String,
    pub target_plan_sha256: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub scenarios: Vec<RestoreScenarioResult>,
    pub unexpected_console_messages: u64,
    pub unexpected_network_failures: u64,
    pub axe_serious_or_critical: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreScenarioResult {
    pub name: String,
    pub succeeded: bool,
}

use ryframe_kernel::{AppError, AppResult};

use crate::ports::jobs::BackgroundJobRecord;

pub(super) fn manual_retry_permission(job_type: &str) -> Option<&'static str> {
    match job_type {
        "system.tenant_config.export" => Some("system:config-package:export"),
        "system.tenant_config.preview" => Some("system:config-transfer:preview"),
        "system.tenant_config.apply" => Some("system:config-transfer:apply"),
        "system.tenant_config.rollback" => Some("system:config-transfer:rollback"),
        _ => None,
    }
}

/// 按任务类型收敛可向用户展示的失败信息。
pub fn public_job_error(job_type: &str, error: Option<String>) -> Option<String> {
    error.map(|error| match job_type {
        "system.tenant_config.export" => "配置包生成失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.preview" => "配置预览失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.apply" => "配置应用失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.rollback" => "配置回滚失败，请稍后重试或联系管理员".to_owned(),
        _ => error,
    })
}

pub(super) fn normalize_job_type_filter(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.len() > 96 {
        return Err(AppError::Validation(
            "任务类型长度必须在 1 到 96 个字节之间".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn normalize_schedule_id_filter(value: Option<String>) -> AppResult<Option<i64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    let schedule_id = value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("来源计划 ID 必须是正整数".into()))?;
    Ok(Some(schedule_id))
}

/// 规范化并校验后台任务状态筛选。
pub fn normalize_job_status_filter(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if !matches!(
        value,
        BackgroundJobRecord::STATUS_PENDING
            | BackgroundJobRecord::STATUS_RUNNING
            | BackgroundJobRecord::STATUS_SUCCEEDED
            | BackgroundJobRecord::STATUS_DEAD
    ) {
        return Err(AppError::Validation(
            "任务状态只能是 pending、running、succeeded 或 dead".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

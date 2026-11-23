use ryframe_application::ports::backup::{RestorePlan, RestoreTargetPreimage};
use ryframe_kernel::{AppError, AppResult};
use std::io::Read;

pub fn required() -> AppError {
    AppError::Validation("恢复目标缺少已绑定的初始化前像，禁止覆盖现有业务数据".into())
}

pub fn read(reader: impl Read) -> AppResult<RestoreTargetPreimage> {
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::Validation("无法读取恢复初始化前像".into()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AppError::Validation("恢复初始化前像不能超过 16 MiB".into()));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Validation("恢复初始化前像必须为 UTF-8".into()))?;
    serde_json::from_str(text)
        .map_err(|_| AppError::Validation("恢复初始化前像必须为一个完整的 JSON 对象".into()))
}

pub fn require_registry_disjoint(
    plan: &RestorePlan,
    server_uuid: &str,
    database: &str,
) -> AppResult<()> {
    if plan.databases.iter().any(|target| {
        target.server_uuid == server_uuid && target.database.eq_ignore_ascii_case(database)
    }) {
        return Err(AppError::Validation(
            "恢复登记库与计划中的隔离目标指向同一个物理数据库".into(),
        ));
    }
    Ok(())
}

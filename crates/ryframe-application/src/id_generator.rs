use std::sync::OnceLock;

use ryframe_kernel::{AppError, AppResult};

pub type BusinessIdGenerator = fn() -> AppResult<i64>;

static ID_GENERATOR: OnceLock<BusinessIdGenerator> = OnceLock::new();

/// 在进程组合根安装应用用例所需的业务 ID 生成器。
pub fn install(generator: BusinessIdGenerator) -> AppResult<()> {
    ID_GENERATOR
        .set(generator)
        .map_err(|_| AppError::Config("应用业务 ID 生成器不能重复安装".into()))
}

/// 生成一个应用业务主键。
pub fn next_id() -> AppResult<i64> {
    ID_GENERATOR
        .get()
        .ok_or_else(|| AppError::Config("应用业务 ID 生成器尚未安装".into()))?()
}

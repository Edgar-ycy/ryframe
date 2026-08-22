use std::sync::OnceLock;

use ryframe_kernel::{AppError, AppResult};

pub type DatabaseIdGenerator = fn() -> AppResult<i64>;

static ID_GENERATOR: OnceLock<DatabaseIdGenerator> = OnceLock::new();

/// 在进程组合根安装数据库写入所需的业务 ID 生成器。
pub fn install(generator: DatabaseIdGenerator) -> AppResult<()> {
    ID_GENERATOR
        .set(generator)
        .map_err(|_| AppError::Config("数据库 ID 生成器不能重复安装".into()))
}

/// 生成一个数据库业务主键。
pub fn next_id() -> AppResult<i64> {
    ID_GENERATOR
        .get()
        .ok_or_else(|| AppError::Config("数据库 ID 生成器尚未安装".into()))?()
}

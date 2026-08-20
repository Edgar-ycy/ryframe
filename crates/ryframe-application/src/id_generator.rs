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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_id() -> AppResult<i64> {
        Ok(43)
    }

    #[test]
    fn installed_generator_is_used_and_cannot_be_replaced() {
        install(fixed_id).expect("首次安装应成功");
        assert_eq!(next_id().expect("ID 应生成成功"), 43);
        assert!(install(fixed_id).is_err());
    }
}

use std::future::Future;

use ryframe_config::MigrationMode;
use ryframe_kernel::AppError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiRunMode {
    Serve,
    Probe,
}

impl ApiRunMode {
    pub const fn starts_background_tasks(self) -> bool {
        matches!(self, Self::Serve)
    }
}

pub fn parse_api_run_mode(arguments: &[String]) -> Result<ApiRunMode, AppError> {
    match arguments {
        [] => Ok(ApiRunMode::Serve),
        [command] if command == "--probe" => Ok(ApiRunMode::Probe),
        _ => Err(AppError::Config("用法: ryframe [--probe]".into())),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerRunMode {
    Continuous,
    Once,
    Probe,
}

impl WorkerRunMode {
    pub const fn allows_initialization_writes(self) -> bool {
        !matches!(self, Self::Probe)
    }
}

pub fn parse_worker_run_mode(arguments: &[String]) -> Result<WorkerRunMode, AppError> {
    match arguments {
        [] => Ok(WorkerRunMode::Continuous),
        [command] if command == "--once" => Ok(WorkerRunMode::Once),
        [command] if command == "--probe" => Ok(WorkerRunMode::Probe),
        _ => Err(AppError::Config(
            "用法: ryframe-worker [--once|--probe]".into(),
        )),
    }
}

/// 探活进程不得执行 DDL；普通服务仍遵循配置的迁移模式。
pub const fn effective_migration_mode(
    allows_initialization_writes: bool,
    configured: MigrationMode,
) -> MigrationMode {
    if !allows_initialization_writes && matches!(configured, MigrationMode::Auto) {
        MigrationMode::Verify
    } else {
        configured
    }
}

/// 普通启动可以建立所需基础设施，探活只允许执行等价的只读校验。
pub async fn provision_or_verify<T, E, P, PF, V, VF>(
    allows_initialization_writes: bool,
    provision: P,
    verify: V,
) -> Result<T, E>
where
    P: FnOnce() -> PF,
    PF: Future<Output = Result<T, E>>,
    V: FnOnce() -> VF,
    VF: Future<Output = Result<T, E>>,
{
    if allows_initialization_writes {
        provision().await
    } else {
        verify().await
    }
}

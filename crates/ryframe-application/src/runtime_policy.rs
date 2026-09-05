use std::{sync::Arc, time::Duration};

use chrono::Duration as ChronoDuration;
use ryframe_kernel::{AppError, AppResult};

/// 登录保护使用的固定运行策略。
#[derive(Clone, Copy, Debug)]
pub struct AuthPolicy {
    pub(crate) max_login_attempts: u32,
    pub(crate) lockout_seconds: u64,
}

impl AuthPolicy {
    pub fn new(max_login_attempts: u32, lockout_duration_minutes: u32) -> AppResult<Self> {
        if max_login_attempts == 0 || lockout_duration_minutes == 0 {
            return Err(AppError::Config("登录保护阈值和锁定时间必须大于零".into()));
        }
        let lockout_seconds = u64::from(lockout_duration_minutes)
            .checked_mul(60)
            .ok_or_else(|| AppError::Config("登录锁定时间超出支持范围".into()))?;
        Ok(Self {
            max_login_attempts,
            lockout_seconds,
        })
    }
}

/// 授权缓存不可用时的处理策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheAvailabilityPolicy {
    Optional,
    Required,
}

impl CacheAvailabilityPolicy {
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
}

/// 后台任务消费循环共享的租约与退避策略。
#[derive(Clone, Debug)]
pub struct JobWorkerPolicy {
    worker_prefix: Option<Arc<str>>,
    pub(crate) lease_duration: ChronoDuration,
    pub(crate) heartbeat_interval: Duration,
    pub(crate) poll_interval: Duration,
    pub(crate) max_idle_poll_interval: Duration,
    pub(crate) lease_recovery_interval: Duration,
    pub(crate) concurrency: usize,
}

impl JobWorkerPolicy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worker_prefix: Option<&str>,
        lease_seconds: u64,
        heartbeat_seconds: u64,
        poll_interval_ms: u64,
        max_idle_poll_interval_ms: u64,
        lease_recovery_interval_seconds: u64,
        concurrency: usize,
    ) -> AppResult<Self> {
        let lease_seconds = i64::try_from(lease_seconds)
            .map_err(|_| AppError::Config("任务租约时长超出支持范围".into()))?;
        if lease_seconds <= 0
            || heartbeat_seconds == 0
            || heartbeat_seconds >= lease_seconds as u64
            || poll_interval_ms == 0
            || max_idle_poll_interval_ms < poll_interval_ms
            || lease_recovery_interval_seconds == 0
            || concurrency == 0
        {
            return Err(AppError::Config("后台任务运行策略无效".into()));
        }
        let worker_prefix = worker_prefix
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Arc::<str>::from);
        Ok(Self {
            worker_prefix,
            lease_duration: ChronoDuration::seconds(lease_seconds),
            heartbeat_interval: Duration::from_secs(heartbeat_seconds),
            poll_interval: Duration::from_millis(poll_interval_ms),
            max_idle_poll_interval: Duration::from_millis(max_idle_poll_interval_ms),
            lease_recovery_interval: Duration::from_secs(lease_recovery_interval_seconds),
            concurrency,
        })
    }

    pub(crate) fn worker_prefix(&self, fallback: &'static str) -> Arc<str> {
        self.worker_prefix
            .as_ref()
            .map_or_else(|| Arc::from(fallback), Arc::clone)
    }
}

/// 数据库驱动调度器的扫描与容量策略。
#[derive(Clone, Copy, Debug)]
pub struct JobSchedulePolicy {
    pub enabled: bool,
    pub(crate) poll_interval: Duration,
    pub(crate) batch_size: usize,
    pub(crate) max_enabled_per_tenant: usize,
}

impl JobSchedulePolicy {
    pub fn new(
        enabled: bool,
        poll_interval_ms: u64,
        batch_size: usize,
        max_enabled_per_tenant: usize,
    ) -> AppResult<Self> {
        if poll_interval_ms == 0 || batch_size == 0 || max_enabled_per_tenant == 0 {
            return Err(AppError::Config("后台调度策略无效".into()));
        }
        Ok(Self {
            enabled,
            poll_interval: Duration::from_millis(poll_interval_ms),
            batch_size,
            max_enabled_per_tenant,
        })
    }
}

/// 后台任务进程的部署模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobWorkerMode {
    Embedded,
    External,
    Disabled,
}

impl JobWorkerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::External => "external",
            Self::Disabled => "disabled",
        }
    }
}

/// 页面概览需要展示的任务运行状态。
#[derive(Clone, Copy, Debug)]
pub struct JobRuntimePolicy {
    pub worker_mode: JobWorkerMode,
    pub scheduler_enabled: bool,
}

/// 导出任务创建与结果保留策略。
#[derive(Clone, Copy, Debug)]
pub struct ExportPolicy {
    pub(crate) default_max_attempts: i32,
    pub(crate) max_rows: usize,
    pub(crate) retention: ChronoDuration,
}

impl ExportPolicy {
    pub fn new(
        default_max_attempts: i32,
        max_rows: usize,
        retention_hours: u32,
    ) -> AppResult<Self> {
        if default_max_attempts <= 0 || max_rows == 0 || retention_hours == 0 {
            return Err(AppError::Config("导出运行策略无效".into()));
        }
        Ok(Self {
            default_max_attempts,
            max_rows,
            retention: ChronoDuration::hours(i64::from(retention_hours)),
        })
    }
}

/// 消息用例实际使用的生命周期与容量边界。
#[derive(Clone, Copy, Debug)]
pub struct MessagingPolicy {
    pub(crate) enabled: bool,
    pub(crate) ticket_ttl_seconds: u64,
    pub(crate) retention_days: u32,
    pub(crate) max_recipients_per_message: u64,
}

impl MessagingPolicy {
    pub fn new(
        enabled: bool,
        ticket_ttl_seconds: u64,
        retention_days: u32,
        max_recipients_per_message: u64,
    ) -> AppResult<Self> {
        if ticket_ttl_seconds == 0 || retention_days == 0 || max_recipients_per_message == 0 {
            return Err(AppError::Config("消息运行策略无效".into()));
        }
        Ok(Self {
            enabled,
            ticket_ttl_seconds,
            retention_days,
            max_recipients_per_message,
        })
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }
}

/// 可恢复用户导入的资源与并发边界。
#[derive(Clone, Copy, Debug)]
pub struct UserImportPolicy {
    pub(crate) max_file_bytes: usize,
    pub(crate) max_rows: usize,
    pub(crate) batch_size: usize,
    pub(crate) max_active_per_tenant: usize,
    pub(crate) hash_parallelism: usize,
}

impl UserImportPolicy {
    pub fn new(
        max_file_bytes: usize,
        max_rows: usize,
        batch_size: usize,
        max_active_per_tenant: usize,
        hash_parallelism: usize,
    ) -> AppResult<Self> {
        if max_file_bytes == 0
            || max_rows == 0
            || batch_size == 0
            || batch_size > max_rows
            || max_active_per_tenant == 0
            || hash_parallelism == 0
        {
            return Err(AppError::Config("用户导入策略无效".into()));
        }
        Ok(Self {
            max_file_bytes,
            max_rows,
            batch_size,
            max_active_per_tenant,
            hash_parallelism,
        })
    }
}

/// 租户配置包和后台传输操作的固定边界。
#[derive(Clone, Copy, Debug)]
pub struct TenantConfigTransferPolicy {
    pub(crate) max_package_bytes: usize,
    pub(crate) max_uncompressed_bytes: usize,
    pub(crate) max_items: usize,
    pub(crate) artifact_hours: u32,
    pub(crate) rollback_hours: u32,
    pub(crate) lease_seconds: u64,
    pub(crate) max_runtime_seconds: u32,
}

impl TenantConfigTransferPolicy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_package_bytes: usize,
        max_uncompressed_bytes: usize,
        max_items: usize,
        artifact_hours: u32,
        rollback_hours: u32,
        lease_seconds: u64,
        max_runtime_seconds: u32,
    ) -> AppResult<Self> {
        if max_package_bytes == 0
            || max_uncompressed_bytes < max_package_bytes
            || max_items == 0
            || artifact_hours == 0
            || rollback_hours == 0
            || lease_seconds == 0
            || max_runtime_seconds == 0
        {
            return Err(AppError::Config("租户配置传输策略无效".into()));
        }
        Ok(Self {
            max_package_bytes,
            max_uncompressed_bytes,
            max_items,
            artifact_hours,
            rollback_hours,
            lease_seconds,
            max_runtime_seconds,
        })
    }
}

/// 多租户启停只在用例边界表达，不携带配置加载细节。
#[derive(Clone, Copy, Debug)]
pub struct MultiTenancyPolicy {
    enabled: bool,
}

impl MultiTenancyPolicy {
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub const fn fixed_tenant_id(self) -> Option<&'static str> {
        if self.enabled { None } else { Some("system") }
    }

    pub fn allows_tenant(self, tenant_id: &str) -> bool {
        self.enabled || tenant_id == "system"
    }
}

/// 校验租户数据目标的稳定键。
pub fn is_valid_tenant_target_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
    (2..=64).contains(&bytes.len())
        && bytes.first().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes.last().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || matches!(byte, b'-' | b'_'))
}

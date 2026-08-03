use serde::Deserialize;

use crate::Environment;

/// 后台任务执行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum JobWorkerMode {
    /// Web 进程内启动 Worker，适合开发环境。
    #[default]
    Embedded,
    /// 仅由独立的 `ryframe-worker` 进程消费任务，适合生产环境。
    External,
    /// 不消费任务；只允许测试环境使用。
    Disabled,
}

/// 持久化后台任务的运行参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobConfig {
    /// Worker 执行模式；未配置时开发/测试使用 embedded，生产使用 external。
    #[serde(default)]
    pub mode: JobWorkerMode,
    /// 空队列时再次轮询前的等待时间。
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// 单次领取任务后的租约时长。
    #[serde(default = "default_lease_seconds")]
    pub lease_seconds: u64,
    /// 任务执行期间续租的心跳间隔。
    #[serde(default = "default_heartbeat_seconds")]
    pub heartbeat_seconds: u64,
    /// 未单独指定重试预算时使用的最大尝试次数。
    #[serde(default = "default_max_attempts")]
    pub default_max_attempts: i32,
    /// 单次导出允许读取的最大记录数。
    #[serde(default = "default_export_max_rows")]
    pub export_max_rows: usize,
    /// 导出结果保留时长，单位为小时。
    #[serde(default = "default_export_retention_hours")]
    pub export_retention_hours: u32,
    /// 单个 Worker 进程内并行消费任务的槽位数。
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// 可选的 Worker 标识前缀；未配置时自动生成。
    #[serde(default)]
    pub worker_id: Option<String>,
    /// 独立 Worker 健康探针监听地址。
    #[serde(default = "default_health_host")]
    pub health_host: String,
    /// 独立 Worker 健康探针与指标端口。
    #[serde(default = "default_health_port")]
    pub health_port: u16,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            mode: JobWorkerMode::Embedded,
            poll_interval_ms: default_poll_interval_ms(),
            lease_seconds: default_lease_seconds(),
            heartbeat_seconds: default_heartbeat_seconds(),
            default_max_attempts: default_max_attempts(),
            export_max_rows: default_export_max_rows(),
            export_retention_hours: default_export_retention_hours(),
            concurrency: default_concurrency(),
            worker_id: None,
            health_host: default_health_host(),
            health_port: default_health_port(),
        }
    }
}

impl JobConfig {
    /// 校验运行参数和环境约束。
    pub fn validate(&self, environment: Environment) -> Result<(), String> {
        if self.poll_interval_ms < 50 || self.poll_interval_ms > 60_000 {
            return Err("jobs.poll_interval_ms 必须在 50 到 60000 之间".into());
        }
        if self.lease_seconds == 0 || self.lease_seconds > 3_600 {
            return Err("jobs.lease_seconds 必须在 1 到 3600 之间".into());
        }
        if self.heartbeat_seconds == 0 || self.heartbeat_seconds >= self.lease_seconds {
            return Err("jobs.heartbeat_seconds 必须大于 0 且小于 jobs.lease_seconds".into());
        }
        if !(1..=100).contains(&self.default_max_attempts) {
            return Err("jobs.default_max_attempts 必须在 1 到 100 之间".into());
        }
        if self.export_max_rows == 0 || self.export_max_rows > 5_000_000 {
            return Err("jobs.export_max_rows 必须在 1 到 5000000 之间".into());
        }
        if self.export_retention_hours == 0 || self.export_retention_hours > 8_760 {
            return Err("jobs.export_retention_hours 必须在 1 到 8760 之间".into());
        }
        if self.concurrency == 0 || self.concurrency > 64 {
            return Err("jobs.concurrency 必须在 1 到 64 之间".into());
        }
        if self
            .worker_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 100)
        {
            return Err("jobs.worker_id 配置后必须为 1 到 100 字节".into());
        }
        if self.health_host.trim().is_empty() {
            return Err("jobs.health_host 不能为空".into());
        }
        if self.health_port == 0 {
            return Err("jobs.health_port 必须大于 0".into());
        }
        if !environment.is_test() && self.mode == JobWorkerMode::Disabled {
            return Err("jobs.mode = \"disabled\" 仅允许测试环境使用".into());
        }
        Ok(())
    }
}

const fn default_poll_interval_ms() -> u64 {
    500
}

const fn default_lease_seconds() -> u64 {
    60
}

const fn default_heartbeat_seconds() -> u64 {
    15
}

const fn default_max_attempts() -> i32 {
    8
}

const fn default_export_max_rows() -> usize {
    500_000
}

const fn default_export_retention_hours() -> u32 {
    24
}

const fn default_concurrency() -> usize {
    4
}

fn default_health_host() -> String {
    "0.0.0.0".into()
}

const fn default_health_port() -> u16 {
    9091
}

#[cfg(test)]
mod tests {
    use super::{Environment, JobConfig, JobWorkerMode};

    #[test]
    fn defaults_match_the_persistent_worker_contract() {
        let config = JobConfig::default();
        assert_eq!(config.lease_seconds, 60);
        assert_eq!(config.heartbeat_seconds, 15);
        assert_eq!(config.default_max_attempts, 8);
        assert_eq!(config.export_max_rows, 500_000);
        assert_eq!(config.export_retention_hours, 24);
        assert!(config.validate(Environment::Dev).is_ok());
    }

    #[test]
    fn heartbeat_and_export_limits_are_fail_closed() {
        let config = JobConfig {
            heartbeat_seconds: JobConfig::default().lease_seconds,
            ..JobConfig::default()
        };
        assert!(config.validate(Environment::Dev).is_err());

        let config = JobConfig {
            default_max_attempts: 0,
            ..JobConfig::default()
        };
        assert!(config.validate(Environment::Dev).is_err());

        let config = JobConfig {
            export_max_rows: 0,
            ..JobConfig::default()
        };
        assert!(config.validate(Environment::Dev).is_err());

        let config = JobConfig {
            export_retention_hours: 0,
            ..JobConfig::default()
        };
        assert!(config.validate(Environment::Dev).is_err());
    }

    #[test]
    fn disabled_mode_is_limited_to_tests() {
        let config = JobConfig::default();
        assert!(config.validate(Environment::Prod).is_ok());

        let config = JobConfig {
            mode: JobWorkerMode::Disabled,
            ..JobConfig::default()
        };
        assert!(config.validate(Environment::Prod).is_err());
        assert!(config.validate(Environment::Test).is_ok());
    }
}

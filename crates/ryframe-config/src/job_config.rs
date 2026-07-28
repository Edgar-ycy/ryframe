use serde::Deserialize;

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
            concurrency: default_concurrency(),
            worker_id: None,
            health_host: default_health_host(),
            health_port: default_health_port(),
        }
    }
}

impl JobConfig {
    /// 校验运行参数和环境约束。
    pub fn validate(&self, environment: &str) -> Result<(), String> {
        if self.poll_interval_ms < 50 || self.poll_interval_ms > 60_000 {
            return Err("jobs.poll_interval_ms 必须在 50 到 60000 之间".into());
        }
        if self.lease_seconds == 0 || self.lease_seconds > 3_600 {
            return Err("jobs.lease_seconds 必须在 1 到 3600 之间".into());
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
        if environment != "test" && self.mode == JobWorkerMode::Disabled {
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
    use super::{JobConfig, JobWorkerMode};

    #[test]
    fn disabled_mode_is_limited_to_tests() {
        let config = JobConfig::default();
        assert!(config.validate("prod").is_ok());

        let config = JobConfig {
            mode: JobWorkerMode::Disabled,
            ..JobConfig::default()
        };
        assert!(config.validate("prod").is_err());
        assert!(config.validate("test").is_ok());
    }
}

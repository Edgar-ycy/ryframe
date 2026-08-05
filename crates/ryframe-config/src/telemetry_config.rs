use serde::Deserialize;

/// OpenTelemetry 导出与采样的强类型配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    /// 是否初始化 OTLP 导出器；关闭时不会影响应用就绪状态。
    #[serde(default)]
    pub enabled: bool,
    /// OTLP/HTTP traces 接收端地址。
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// 资源中的服务名称。
    #[serde(default = "default_service_name")]
    pub service_name: String,
    /// 0 到 1 之间的根 span 采样比例。
    #[serde(default = "default_sample_ratio")]
    pub sample_ratio: f64,
    /// 单次 OTLP 导出的最大等待时间。
    #[serde(default = "default_export_timeout_secs")]
    pub export_timeout_secs: u64,
    /// 批量导出前允许暂存的最大 span 数。
    #[serde(default = "default_max_queue_size")]
    pub max_queue_size: usize,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_endpoint(),
            service_name: default_service_name(),
            sample_ratio: default_sample_ratio(),
            export_timeout_secs: default_export_timeout_secs(),
            max_queue_size: default_max_queue_size(),
        }
    }
}

impl TelemetryConfig {
    /// 校验会影响导出器安全性和资源消耗的配置边界。
    pub fn validate(&self) -> Result<(), String> {
        if self.service_name.trim().is_empty() || self.service_name.len() > 128 {
            return Err("telemetry.service_name 必须为 1 到 128 个字符".into());
        }
        if self.enabled && self.endpoint.trim().is_empty() {
            return Err("telemetry.enabled=true 时 telemetry.endpoint 不能为空".into());
        }
        if !self.sample_ratio.is_finite() || !(0.0..=1.0).contains(&self.sample_ratio) {
            return Err("telemetry.sample_ratio 必须在 0 到 1 之间".into());
        }
        if self.export_timeout_secs == 0 || self.export_timeout_secs > 60 {
            return Err("telemetry.export_timeout_secs 必须在 1 到 60 之间".into());
        }
        if self.max_queue_size == 0 || self.max_queue_size > 65_536 {
            return Err("telemetry.max_queue_size 必须在 1 到 65536 之间".into());
        }
        Ok(())
    }
}

fn default_endpoint() -> String {
    "http://localhost:4318/v1/traces".into()
}

fn default_service_name() -> String {
    "ryframe".into()
}

const fn default_sample_ratio() -> f64 {
    0.1
}

const fn default_export_timeout_secs() -> u64 {
    5
}

const fn default_max_queue_size() -> usize {
    2048
}

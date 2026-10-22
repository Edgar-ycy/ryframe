use std::{collections::BTreeMap, fmt};

use serde::Deserialize;

mod span_limits;
mod validation;

pub use span_limits::TelemetrySpanLimits;

/// OpenTelemetry 导出与采样的强类型配置。
#[derive(Clone, Deserialize)]
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
    /// OTLP 请求头；名称不区分大小写且不能覆盖运输层管理字段。
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 用户资源属性；不能覆盖服务、构建或 SDK 身份。
    #[serde(default)]
    pub resource_attributes: BTreeMap<String, String>,
    /// 批量导出的调度间隔，单位为毫秒。
    #[serde(default = "default_scheduled_delay_millis")]
    pub scheduled_delay_millis: u64,
    /// 批处理器等待单批导出完成的时限，单位为秒。
    #[serde(default = "default_batch_export_timeout_secs")]
    pub batch_export_timeout_secs: u64,
    /// 同时执行的最大导出批次数。
    #[serde(default = "default_max_concurrent_exports")]
    pub max_concurrent_exports: usize,
    /// 单批最大 span 数；省略时取队列容量与 512 的较小值。
    #[serde(default)]
    pub max_export_batch_size: Option<usize>,
    /// 单个 span 及其事件、链接的容量限制。
    #[serde(default)]
    pub span_limits: TelemetrySpanLimits,
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
            headers: BTreeMap::new(),
            resource_attributes: BTreeMap::new(),
            scheduled_delay_millis: default_scheduled_delay_millis(),
            batch_export_timeout_secs: default_batch_export_timeout_secs(),
            max_concurrent_exports: default_max_concurrent_exports(),
            max_export_batch_size: None,
            span_limits: TelemetrySpanLimits::default(),
        }
    }
}

impl fmt::Debug for TelemetryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryConfig")
            .field("enabled", &self.enabled)
            .field("endpoint", &"[REDACTED]")
            .field("service_name", &self.service_name)
            .field("sample_ratio", &self.sample_ratio)
            .field("export_timeout_secs", &self.export_timeout_secs)
            .field("max_queue_size", &self.max_queue_size)
            .field("headers", &"[REDACTED]")
            .field("resource_attributes", &"[REDACTED]")
            .field("scheduled_delay_millis", &self.scheduled_delay_millis)
            .field("batch_export_timeout_secs", &self.batch_export_timeout_secs)
            .field("max_concurrent_exports", &self.max_concurrent_exports)
            .field("max_export_batch_size", &self.max_export_batch_size)
            .field("span_limits", &self.span_limits)
            .finish()
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

const fn default_scheduled_delay_millis() -> u64 {
    5000
}

const fn default_batch_export_timeout_secs() -> u64 {
    30
}

const fn default_max_concurrent_exports() -> usize {
    1
}

use serde::Deserialize;

/// Span、事件与链接的容量限制；零表示不保留对应条目。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetrySpanLimits {
    /// 每个 span 的最大属性数。
    pub max_attributes_per_span: u32,
    /// 每个 span 的最大事件数。
    pub max_events_per_span: u32,
    /// 每个 span 的最大链接数。
    pub max_links_per_span: u32,
    /// 每个事件的最大属性数。
    pub max_attributes_per_event: u32,
    /// 每个链接的最大属性数。
    pub max_attributes_per_link: u32,
}

impl Default for TelemetrySpanLimits {
    fn default() -> Self {
        Self {
            max_attributes_per_span: 128,
            max_events_per_span: 128,
            max_links_per_span: 128,
            max_attributes_per_event: 128,
            max_attributes_per_link: 128,
        }
    }
}

impl TelemetrySpanLimits {
    pub(super) fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("max_attributes_per_span", self.max_attributes_per_span),
            ("max_events_per_span", self.max_events_per_span),
            ("max_links_per_span", self.max_links_per_span),
            ("max_attributes_per_event", self.max_attributes_per_event),
            ("max_attributes_per_link", self.max_attributes_per_link),
        ] {
            if value > 65_536 {
                return Err(format!(
                    "telemetry.span_limits.{field} 超出容量范围，请设置为 0 到 65536"
                ));
            }
        }
        Ok(())
    }
}

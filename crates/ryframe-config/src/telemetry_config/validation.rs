use std::collections::BTreeSet;

use super::TelemetryConfig;

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
        self.validate_batch()?;
        self.span_limits.validate()?;
        self.validate_headers()?;
        self.validate_resource_attributes()
    }

    fn validate_batch(&self) -> Result<(), String> {
        if !(1..=600_000).contains(&self.scheduled_delay_millis) {
            return Err("telemetry.scheduled_delay_millis 超出范围，请设置为 1 到 600000".into());
        }
        if !(1..=60).contains(&self.batch_export_timeout_secs) {
            return Err("telemetry.batch_export_timeout_secs 超出范围，请设置为 1 到 60".into());
        }
        if !(1..=64).contains(&self.max_concurrent_exports) {
            return Err("telemetry.max_concurrent_exports 超出范围，请设置为 1 到 64".into());
        }
        if self
            .max_export_batch_size
            .is_some_and(|size| size == 0 || size > self.max_queue_size)
        {
            return Err(
                "telemetry.max_export_batch_size 超出范围，请设置为 1 到 telemetry.max_queue_size 或省略此字段"
                    .into(),
            );
        }
        Ok(())
    }

    fn validate_headers(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        for (name, value) in &self.headers {
            if name.is_empty() || !name.bytes().all(is_header_name_byte) {
                return Err("telemetry.headers 名称无效，请使用非空的 ASCII HTTP token".into());
            }
            let normalized = name.to_ascii_lowercase();
            if !names.insert(normalized.clone()) {
                return Err("telemetry.headers 名称重复，请合并大小写不同的同名请求头".into());
            }
            if matches!(
                normalized.as_str(),
                "host"
                    | "content-length"
                    | "content-type"
                    | "content-encoding"
                    | "transfer-encoding"
                    | "connection"
                    | "keep-alive"
                    | "proxy-connection"
                    | "te"
                    | "trailer"
                    | "upgrade"
            ) {
                return Err("telemetry.headers 包含运输层管理字段，请移除该请求头".into());
            }
            if !value
                .bytes()
                .all(|byte| byte == b'\t' || (byte >= 32 && byte != 127))
            {
                return Err(
                    "telemetry.headers 值含非法控制字符，请移除 CR、LF 及其他禁止字符".into(),
                );
            }
        }
        Ok(())
    }

    fn validate_resource_attributes(&self) -> Result<(), String> {
        for (key, value) in &self.resource_attributes {
            if key.trim().is_empty() || key.chars().any(char::is_control) {
                return Err(
                    "telemetry.resource_attributes 键无效，请使用非空且不含控制字符的键".into(),
                );
            }
            if value.chars().any(char::is_control) {
                return Err("telemetry.resource_attributes 值无效，请移除控制字符".into());
            }
            if matches!(
                key.as_str(),
                "service.name" | "service.version" | "vcs.ref.head.revision"
            ) || key.starts_with("telemetry.sdk.")
            {
                return Err("telemetry.resource_attributes 包含系统身份字段，请移除该属性".into());
            }
        }
        Ok(())
    }
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

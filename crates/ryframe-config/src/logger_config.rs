use serde::Deserialize;

/// 日志级别。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggerLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LoggerLevel {
    /// 返回 `tracing_subscriber` 使用的过滤指令。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// 本地日志编码格式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggerFormat {
    #[default]
    Text,
    Json,
}

/// 本地日志输出目标。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggerOutput {
    #[default]
    Stdout,
    File,
}

/// 日志配置
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggerConfig {
    /// 日志级别：trace / debug / info / warn / error
    pub level: LoggerLevel,
    /// 输出格式：text / json
    pub format: LoggerFormat,
    /// 输出目标：stdout / file
    pub output: LoggerOutput,
    /// 每日滚动文件的最大保留数量，仅在 file 输出时生效
    pub retention_days: usize,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: LoggerLevel::Info,
            format: LoggerFormat::Text,
            output: LoggerOutput::Stdout,
            retention_days: 7,
        }
    }
}

impl LoggerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=3_650).contains(&self.retention_days) {
            return Err("logger.retention_days 必须在 1 到 3650 之间".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LoggerConfig, LoggerFormat, LoggerLevel, LoggerOutput};

    #[test]
    fn default_logger_policy_is_valid_and_bounded() {
        let config = LoggerConfig::default();
        assert_eq!(config.level, LoggerLevel::Info);
        assert_eq!(config.format, LoggerFormat::Text);
        assert_eq!(config.output, LoggerOutput::Stdout);
        assert_eq!(config.retention_days, 7);
        config.validate().unwrap();
    }

    #[test]
    fn invalid_logger_enum_values_are_rejected_during_deserialization() {
        for invalid in [
            "level = \"verbose\"\nformat = \"text\"\noutput = \"stdout\"\nretention_days = 7",
            "level = \"info\"\nformat = \"pretty\"\noutput = \"stdout\"\nretention_days = 7",
            "level = \"info\"\nformat = \"text\"\noutput = \"stderr\"\nretention_days = 7",
        ] {
            assert!(toml::from_str::<LoggerConfig>(invalid).is_err());
        }
    }

    #[test]
    fn invalid_retention_is_rejected() {
        let config = LoggerConfig {
            retention_days: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = LoggerConfig {
            retention_days: 3_651,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}

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

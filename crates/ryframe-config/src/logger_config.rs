use serde::Deserialize;

/// 日志配置
#[derive(Debug, Clone, Deserialize)]
pub struct LoggerConfig {
    /// 日志级别：trace / debug / info / warn / error
    pub level: String,
    /// 输出格式：text / json
    pub format: String,
    /// 输出目标：stdout / file
    pub output: String,
}

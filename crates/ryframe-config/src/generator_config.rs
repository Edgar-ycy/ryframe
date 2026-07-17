use serde::Deserialize;

/// 代码生成器配置。
#[derive(Debug, Clone, Deserialize)]
pub struct GeneratorConfig {
    /// 用于读取表结构的数据源名称；`primary` 表示主库。
    pub data_source: String,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            data_source: "primary".into(),
        }
    }
}

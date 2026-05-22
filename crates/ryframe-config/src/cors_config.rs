use serde::Deserialize;

/// CORS 配置
///
/// 在配置文件中通过 `[cors]` section 配置允许的源。
/// 未配置时默认使用 `mirror_request` 模式（兼容 credentials，适合开发环境）。
#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    /// 允许的来源列表（逗号分隔的字符串列表）
    ///
    /// 示例：`["http://localhost:5173", "http://localhost:3000"]`
    /// 为空时使用 mirror_request 模式（回显请求 Origin，兼容 credentials）
    #[serde(default)]
    pub allow_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origins: Vec::new(),
        }
    }
}

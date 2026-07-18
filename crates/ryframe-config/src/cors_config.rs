use serde::Deserialize;

/// CORS 配置
///
/// 在配置文件中通过 `[cors]` section 配置允许的源。
/// 空列表不会回显请求 Origin，因此拒绝所有跨域访问。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    /// 允许的来源列表（逗号分隔的字符串列表）
    ///
    /// 示例：`["http://localhost:80", "http://localhost:3000"]`
    /// 为空时拒绝跨域请求。
    #[serde(default)]
    pub allow_origins: Vec<String>,
}

use serde::Deserialize;

/// SQL 日志输出级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SqlLogLevel {
    /// 关闭 SQL 日志
    #[default]
    Off,
    /// 仅输出 SQL 语句 + 耗时 + 返回行数
    Summary,
    /// 完整输出（含结果行数详情）
    Full,
}

/// 数据库配置
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// SQL 日志级别（默认 off）
    #[serde(default)]
    pub sql_log_level: SqlLogLevel,
    /// 数据库连接列表（第一个为主库，后续为额外数据源）
    ///
    /// 示例：
    /// ```toml
    /// [[database.connections]]
    /// driver = "mysql"
    /// host = "localhost"
    /// port = 3306
    /// database = "ryframe_config"
    /// username = "root"
    /// password = "123456"
    /// max_connections = 10
    /// min_connections = 1
    ///
    /// [[database.connections]]
    /// driver = "mysql"
    /// host = "localhost"
    /// port = 3306
    /// database = "ryframe_device"
    /// username = "root"
    /// password = "123456"
    /// max_connections = 5
    /// min_connections = 1
    /// ```
    pub connections: Vec<DbConnection>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            sql_log_level: SqlLogLevel::default(),
            connections: Vec::new(),
        }
    }
}

/// 数据库连接参数
///
/// 连接池调优参考：
/// - **max_connections**: 公式 ≈ (core_count * 2) + effective_spindle_count，通常 10~50
/// - **min_connections**: 保持 1~4 条空闲连接以应对突发流量
/// - **acquire_timeout_secs**: 获取连接超时，建议 5~30 秒
/// - **idle_timeout_secs**: 空闲连接存活时间，建议 300~600 秒
/// - **max_lifetime_secs**: 连接最大生命周期（需 < MySQL wait_timeout），建议 1800~3600 秒
/// - **connect_timeout_secs**: TCP 连接建立超时，建议 3~10 秒
#[derive(Debug, Clone, Deserialize)]
pub struct DbConnection {
    /// 数据库驱动类型：postgres / mysql / sqlite
    pub driver: String,
    /// 主机地址
    pub host: String,
    /// 端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 用户名
    pub username: String,
    /// 密码
    pub password: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 最小连接数（空闲连接池保留数）
    pub min_connections: u32,
    /// 获取连接超时（秒），默认 10
    #[serde(default = "default_acquire_timeout")]
    pub acquire_timeout_secs: u64,
    /// 空闲连接超时（秒），默认 600（10 分钟）
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
    /// 连接最大生命周期（秒），默认 1800（30 分钟）
    #[serde(default = "default_max_lifetime")]
    pub max_lifetime_secs: u64,
    /// 连接建立超时（秒），默认 10
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

fn default_acquire_timeout() -> u64 {
    10
}
fn default_idle_timeout() -> u64 {
    600
}
fn default_max_lifetime() -> u64 {
    1800
}
fn default_connect_timeout() -> u64 {
    10
}

impl DbConnection {
    /// 生成 SeaORM 连接字符串
    ///
    /// 示例输出：
    /// - postgres: "postgres://postgres:password@localhost:5432/ryframe"
    /// - mysql: "mysql://root:password@localhost:3306/ryframe"
    /// - sqlite: "sqlite://data.db?mode=rwc"
    pub fn connection_url(&self) -> String {
        match self.driver.as_str() {
            "sqlite" => format!("sqlite://{}?mode=rwc", self.database),
            "mysql" => format!(
                "{}://{}:{}@{}:{}/{}?collation=utf8mb4_general_ci",
                self.driver, self.username, self.password, self.host, self.port, self.database
            ),
            _ => format!(
                "{}://{}:{}@{}:{}/{}",
                self.driver, self.username, self.password, self.host, self.port, self.database
            ),
        }
    }
}

impl Default for DbConnection {
    fn default() -> Self {
        Self {
            driver: "mysql".into(),
            host: "localhost".into(),
            port: 3306,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            max_connections: 10,
            min_connections: 1,
            acquire_timeout_secs: 10,
            idle_timeout_secs: 600,
            max_lifetime_secs: 1800,
            connect_timeout_secs: 10,
        }
    }
}

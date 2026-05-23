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
    /// 主库连接
    pub primary: DbConnection,
    /// 从库连接（读写分离，可选）
    #[serde(default)]
    pub replicas: Vec<DbConnection>,
    /// 多数据源 — 命名数据源（可选）
    ///
    /// 配合 `#[datasource("name")]` 注解使用。
    ///
    /// 示例：
    /// ```toml
    /// [[database.datasources]]
    /// name = "db_device"
    /// driver = "mysql"
    /// host = "localhost"
    /// port = 3306
    /// database = "ryframe_device"
    /// username = "root"
    /// password = "123456"
    /// max_connections = 5
    /// min_connections = 1
    /// ```
    #[serde(default)]
    pub datasources: Vec<NamedDataSource>,
}

/// 命名数据源
///
/// `name` 字段唯一标识数据源，其余字段与 `DbConnection` 一致。
#[derive(Debug, Clone, Deserialize)]
pub struct NamedDataSource {
    /// 数据源唯一名称，用于 `#[datasource("name")]` 注解引用
    pub name: String,
    #[serde(flatten)]
    pub connection: DbConnection,
}

/// 数据库连接参数
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
            _ => format!(
                "{}://{}:{}@{}:{}/{}",
                self.driver, self.username, self.password, self.host, self.port, self.database
            ),
        }
    }
}

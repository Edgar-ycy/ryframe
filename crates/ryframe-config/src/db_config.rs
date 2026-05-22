use serde::Deserialize;

/// 数据库配置
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// 主库连接
    pub primary: DbConnection,
    /// 从库连接（读写分离，可选）
    #[serde(default)]
    pub replicas: Vec<DbConnection>,
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
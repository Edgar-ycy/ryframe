use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbTlsMode {
    #[default]
    Disabled,
    Required,
    VerifyCa,
    VerifyIdentity,
}

impl DbTlsMode {
    const fn as_sqlx_value(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Required => "required",
            Self::VerifyCa => "verify_ca",
            Self::VerifyIdentity => "verify_identity",
        }
    }
}

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

/// 控制应用进程是否可以执行数据库结构迁移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MigrationMode {
    /// 执行待处理迁移、写入幂等系统数据并校验数据库结构。
    #[default]
    Auto,
    /// 仅校验迁移记录和数据库结构，绝不执行 DDL。
    Verify,
    /// 禁用迁移检查；该模式仅限测试环境使用。
    Off,
}

/// 数据库拓扑配置。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// SQL 日志级别（默认 off）。
    #[serde(default)]
    pub sql_log_level: SqlLogLevel,
    /// 启动时的迁移行为；省略时开发/测试环境默认 `auto`，生产环境默认 `verify`。
    #[serde(default)]
    pub migration_mode: MigrationMode,
    /// 唯一写库，也是无从库时的读库。
    pub primary: DbConnection,
    /// 可选只读副本；读取按配置顺序轮询。
    #[serde(default)]
    pub replicas: Vec<DatabaseReplicaConfig>,
    /// 可选命名业务数据源；必须由具体用例显式选择。
    #[serde(default)]
    pub sources: Vec<DatabaseSourceConfig>,
}

/// 一个命名只读副本。
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseReplicaConfig {
    /// 用于日志、健康检查和故障定位的唯一名称。
    pub name: String,
    #[serde(flatten)]
    pub connection: DbConnection,
}

/// 一个命名业务数据源。
///
/// 业务数据源可以使用与主库不同的结构和驱动，不参与主库查询的读写路由。
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSourceConfig {
    /// 用于依赖注入、日志和健康检查的唯一名称。
    pub name: String,
    #[serde(flatten)]
    pub connection: DbConnection,
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
#[serde(deny_unknown_fields)]
pub struct DbConnection {
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
    /// TLS 策略；远程生产数据库必须使用 `verify_identity`。
    #[serde(default)]
    pub tls_mode: DbTlsMode,
    /// 用于验证服务端的 PEM CA 证书。
    #[serde(default)]
    pub tls_ca: Option<String>,
    /// 可选的 mTLS 客户端证书。
    #[serde(default)]
    pub tls_client_cert: Option<String>,
    /// 可选的 mTLS 客户端私钥。
    #[serde(default)]
    pub tls_client_key: Option<String>,
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
    /// 生成 MySQL SeaORM 连接字符串。
    pub fn connection_url(&self) -> String {
        let username = utf8_percent_encode(&self.username, NON_ALPHANUMERIC);
        let password = utf8_percent_encode(&self.password, NON_ALPHANUMERIC);
        let mut url = format!(
            "mysql://{}:{}@{}:{}/{}?collation=utf8mb4_general_ci&ssl-mode={}",
            username,
            password,
            self.host,
            self.port,
            self.database,
            self.tls_mode.as_sqlx_value()
        );
        for (name, path) in [
            ("ssl-ca", self.tls_ca.as_deref()),
            ("ssl-cert", self.tls_client_cert.as_deref()),
            ("ssl-key", self.tls_client_key.as_deref()),
        ] {
            if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
                url.push('&');
                url.push_str(name);
                url.push('=');
                url.push_str(&utf8_percent_encode(path, NON_ALPHANUMERIC).to_string());
            }
        }
        url
    }
}

impl Default for DbConnection {
    fn default() -> Self {
        Self {
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
            tls_mode: DbTlsMode::Disabled,
            tls_ca: None,
            tls_client_cert: None,
            tls_client_key: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_url_percent_encodes_credentials() {
        let connection = DbConnection {
            host: "127.0.0.1".into(),
            port: 3306,
            database: "ryframe-test".into(),
            username: "user@name".into(),
            password: "p:a/s#%".into(),
            ..DbConnection::default()
        };

        assert_eq!(
            connection.connection_url(),
            "mysql://user%40name:p%3Aa%2Fs%23%25@127.0.0.1:3306/ryframe-test?collation=utf8mb4_general_ci&ssl-mode=disabled"
        );
    }

    #[test]
    fn connection_url_includes_tls_identity_and_client_certificates() {
        let connection = DbConnection {
            host: "db.example.com".into(),
            database: "ryframe".into(),
            username: "app".into(),
            tls_mode: DbTlsMode::VerifyIdentity,
            tls_ca: Some("certs/ca.pem".into()),
            tls_client_cert: Some("certs/client.pem".into()),
            tls_client_key: Some("certs/client.key".into()),
            ..DbConnection::default()
        };

        let url = connection.connection_url();
        assert!(url.contains("ssl-mode=verify_identity"));
        assert!(url.contains("ssl-ca=certs%2Fca%2Epem"));
        assert!(url.contains("ssl-cert=certs%2Fclient%2Epem"));
        assert!(url.contains("ssl-key=certs%2Fclient%2Ekey"));
    }
}

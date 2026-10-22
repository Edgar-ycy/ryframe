use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ryframe_config::{AppConfig, Environment};
use ryframe_kernel::AppError;

static NEXT: AtomicU64 = AtomicU64::new(0);

pub const PRODUCTION: &str = r#"
scope_id = "production-file-contract"
snowflake_worker_id = 8
[auth]
jwt_secret = "production-jwt-secret-must-remain-private-123456789"
[monitor]
metrics_bearer_token = "production-metrics-token-must-remain-private-123456789"
[api_docs]
enabled = false
[redis]
mode = "required"
password = "production-redis-private"
[cors]
allow_origins = ["https://example.com"]
[object_storage]
backend = "s3"
endpoint = "https://storage.example.com"
access_key = "production-access-private"
secret_key = "production-storage-private"
use_ssl = true
"#;

pub struct Fixture {
    pub directory: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local-tests/config-file-contracts")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("app.toml"),
            include_str!("../../../../config/app.toml"),
        )
        .unwrap();
        Self { directory }
    }

    pub fn write(&self, environment: Environment, text: &str) {
        fs::write(self.directory.join(format!("app.{environment}.toml")), text).unwrap();
    }

    pub fn load(&self, environment: Environment) -> AppConfig {
        AppConfig::load(&self.directory, environment).expect("文件配置应有效")
    }

    pub fn error(&self, environment: Environment) -> String {
        match AppConfig::load(&self.directory, environment).unwrap_err() {
            AppError::Config(message) => message,
            other => panic!("应为配置错误：{other:?}"),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

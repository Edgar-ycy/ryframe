//! 配置文件热加载
//!
//! 监听 config/ 目录下的 TOML 文件变更，自动重载可热更新配置项。
//!
//! # 可热更新配置
//! - logger.level / logger.format / logger.output
//! - rate_limit.*
//! - cors.*
//! - redis.*
//!
//! # 不可热更新（需重启）
//! - database.*
//! - app.host / app.port
//! - auth.jwt_secret

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ryframe_config::AppConfig;
use tokio::sync::RwLock;

/// 配置变更回调类型
pub type ConfigChangeCallback = Arc<dyn Fn(&AppConfig) + Send + Sync>;

/// 共享的可热更新配置句柄
///
/// 启动时创建，包装 `Arc<RwLock<AppConfig>>`，
/// 业务代码通过 `self.config.read().await` 读取最新配置。
#[derive(Clone)]
pub struct HotConfig {
    inner: Arc<RwLock<AppConfig>>,
}

impl HotConfig {
    /// 包装现有配置
    pub fn new(config: AppConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
        }
    }

    /// 读取当前配置快照（克隆）
    pub async fn read(&self) -> AppConfig {
        self.inner.read().await.clone()
    }

    /// 获取内部 Arc（用于注入到需要 `Arc<AppConfig>` 的地方）
    pub fn arc(&self) -> Arc<RwLock<AppConfig>> {
        self.inner.clone()
    }

    /// 仅更新可热更新的字段
    pub async fn apply_hot(&self, hot_config: &AppConfig) {
        let mut current = self.inner.write().await;

        // 可热更新字段
        current.logger = hot_config.logger.clone();
        current.rate_limit = hot_config.rate_limit.clone();
        current.cors = hot_config.cors.clone();
        current.redis = hot_config.redis.clone();
    }
}

/// 启动配置文件热加载后台任务
///
/// 每 5 秒检查一次配置文件的修改时间，
/// 检测到变更后重新加载并触发回调。
///
/// # 参数
/// - `hot_config`: 共享的可热更新配置句柄
/// - `config_dir`: 配置文件目录路径
/// - `on_change`: 配置变更回调（例如重新设置 tracing 日志级别）
pub fn spawn_config_watcher(
    hot_config: HotConfig,
    config_dir: String,
    on_change: Option<ConfigChangeCallback>,
) {
    let config_dir_path = PathBuf::from(config_dir);
    let env = std::env::var("APP_ENV").unwrap_or_else(|_| "dev".to_string());

    tokio::spawn(async move {
        let mut last_modified = get_max_modified(&config_dir_path, &env).await;

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

            let current_modified = get_max_modified(&config_dir_path, &env).await;

            if current_modified > last_modified {
                tracing::info!(
                    "检测到配置文件变更 (mtime: {:?})，执行热加载...",
                    current_modified
                );

                let config_dir_str = config_dir_path.to_str().unwrap_or("config");

                match AppConfig::reload_hot(config_dir_str) {
                    Ok(new_config) => {
                        hot_config.apply_hot(&new_config).await;
                        tracing::info!(
                            "配置热加载成功！日志级别: {}, 限流容量: {}/{}",
                            new_config.logger.level,
                            new_config.rate_limit.capacity,
                            new_config.rate_limit.refill_per_sec,
                        );

                        // 触发回调
                        if let Some(ref callback) = on_change {
                            callback(&new_config);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("配置热加载失败（将保持当前配置）: {}", e);
                    }
                }

                last_modified = current_modified;
            }
        }
    });
}

/// 获取指定目录下配置文件中最大的修改时间
async fn get_max_modified(config_dir: &Path, env: &str) -> Option<std::time::SystemTime> {
    let files = [
        config_dir.join("app.toml"),
        config_dir.join(format!("app.{}.toml", env)),
    ];

    let mut max_time: Option<std::time::SystemTime> = None;

    for path in &files {
        if let Ok(meta) = tokio::fs::metadata(path).await
            && let Ok(modified) = meta.modified()
        {
            max_time = Some(max_time.map_or(modified, |t| t.max(modified)));
        }
    }

    max_time
}

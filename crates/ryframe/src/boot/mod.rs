//! 应用启动引导模块
//!
//! 将 `main.rs` 中的初始化逻辑拆分为独立子模块，职责如下：
//! - `logging`:    日志系统 / OpenTelemetry 链路追踪
//! - `datasource`: 应用数据库连接 / 健康检查 / 表校验
//! - `redis`:      Redis 客户端 / Token 黑名单
//! - `services`:   全部 Service 实例构造
//! - `limiter`:    限流器（Redis / 内存双模式）
//! - `storage`:    对象存储（Local / RustFS / MinIO / S3）
//! - `app_state`:  AppState 聚合

pub mod access_catalog;
#[cfg(feature = "bin-api")]
pub mod app_state;
pub mod application_policy;
pub mod artifact_store;
pub mod authorization_cache;
pub mod authorization_cache_keyspace;
pub mod background_services;
#[cfg(feature = "bin-api")]
pub mod backup;
pub mod control_plane;
pub mod datasource;
pub mod file_content;
#[cfg(feature = "bin-api")]
pub mod idempotency;
pub mod jobs;
#[cfg(feature = "bin-api")]
pub mod limiter;
pub mod logging;
#[cfg(feature = "bin-api")]
pub mod login_protection;
#[cfg(feature = "bin-api")]
pub mod message_listener;
#[cfg(feature = "bin-api")]
pub mod online_sessions;
pub mod readiness;
#[cfg(feature = "bin-api")]
pub mod redis;
#[cfg(feature = "bin-api")]
pub mod refresh_sessions;
#[cfg(feature = "bin-api")]
pub mod services;
#[cfg(feature = "bin-api")]
pub mod session_security;
pub mod spreadsheet;
#[doc(hidden)]
pub mod startup;
#[cfg(feature = "bin-api")]
pub mod storage;
pub mod tenant_config_archive;
pub mod tenant_data;

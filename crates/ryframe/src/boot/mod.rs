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

pub mod agent_limiter;
pub mod app_state;
pub mod application_policy;
pub mod artifact_store;
pub mod authorization_cache;
mod authorization_cache_keyspace;
pub mod datasource;
pub mod file_content;
pub mod idempotency;
pub mod jobs;
pub mod limiter;
pub mod logging;
pub mod message_listener;
pub mod readiness;
pub mod redis;
pub mod refresh_sessions;
pub mod services;
pub mod session_security;
pub mod spreadsheet;
pub mod storage;
pub mod tenant_data;

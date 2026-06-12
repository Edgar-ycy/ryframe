//! 应用启动引导模块
//!
//! 将 `main.rs` 中的初始化逻辑拆分为独立子模块，职责如下：
//! - `logging`:    日志系统 / OpenTelemetry 链路追踪
//! - `datasource`: 多数据源连接 / 健康检查 / 表校验
//! - `redis`:      Redis 客户端 / Token 黑名单
//! - `services`:   全部 Service 实例构造
//! - `limiter`:    限流器（Redis / 内存双模式）
//! - `storage`:    对象存储（Local / MinIO / S3）
//! - `app_state`:  AppState 聚合

pub mod app_state;
pub mod datasource;
pub mod limiter;
pub mod logging;
pub mod redis;
pub mod services;
pub mod storage;

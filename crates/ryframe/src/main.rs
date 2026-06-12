mod app;
mod boot;

use std::{net::SocketAddr, sync::Arc};

use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::{AppContext, HotConfig, spawn_config_watcher};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // 1. 加载配置
    let config = AppConfig::load("config")?;
    tracing::info!(
        "配置加载完成, 环境: {}",
        std::env::var("APP_ENV").unwrap_or_else(|_| "dev".into())
    );
    let hot_config = HotConfig::new(config.clone());

    // 2. 初始化日志 + OpenTelemetry
    let (_logger_guard, _telemetry_guard) = boot::logging::init(&config);
    tracing::info!(
        "日志系统初始化完成, 级别: {}, 输出: {}",
        config.logger.level,
        config.logger.output
    );

    // 3. 启动进程指标采集
    ryframe_middleware::metrics::spawn_process_metrics_updater();

    // 4. 连接数据库 + 健康检查 + 表校验
    let ds = boot::datasource::connect(&config).await?;

    // 5. 创建应用上下文
    let context = AppContext::new(config.clone());
    let config_arc = Arc::new(config.clone());

    // 6. 初始化 Redis + Token 黑名单
    let redis = boot::redis::init(&config.redis).await;

    // 7. 构造所有 Service（含调度器 + 内置任务注册）
    let services = boot::services::build_all(&config, &redis.client, &ds.primary).await?;

    // 8. 初始化限流器
    let limit = boot::limiter::init(&config, &redis.client);

    // 9. 初始化对象存储
    let object_storage = boot::storage::init(&config);

    // 10. 提前提取 scheduler (在 state move 之前)
    let scheduler = services.scheduler.clone();

    // 11. 聚合 AppState + 构建 Router
    let state = boot::app_state::assemble(
        ds.primary.clone(),
        ds.extras,
        config_arc,
        context,
        redis.client.clone(),
        redis.token_blacklist,
        services,
        limit.limiter.clone(),
        object_storage,
    );
    let router = app::build_app(state, limit.limiter, limit.rate_limit_state, &config.cors);

    // 12. 启动 Scheduler 后台
    scheduler.clone().spawn();
    tracing::info!("TaskScheduler 已启动");

    // 13. 启动 HTTP 服务
    let addr = format!("{}:{}", config.app.host, config.app.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| AppError::Internal(format!("绑定地址 {} 失败: {}", addr, e)))?;
    tracing::info!("服务启动: http://{}", addr);

    // 14. 启动配置文件热加载
    spawn_config_watcher(
        hot_config,
        "config".to_string(),
        Some(Arc::new(move |new_config: &AppConfig| {
            tracing::info!(
                "[ConfigWatcher] 配置已热更新 - 日志级别: {}, 限流: {}/{}",
                new_config.logger.level,
                new_config.rate_limit.capacity,
                new_config.rate_limit.refill_per_sec,
            );
        })),
    );

    // 使用 tokio::select 同时等待服务和停机信号
    tokio::select! {
        result = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(shutdown_signal()) =>
        {
            result.map_err(|e| AppError::Internal(format!("服务异常退出: {}", e)))?;
        }
    }

    scheduler.shutdown();
    tracing::info!("服务已停止");
    Ok(())
}

/// 优雅停机信号
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("收到停机信号，开始优雅停机...");
}

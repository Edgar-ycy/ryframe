mod app;

use std::{net::SocketAddr, sync::Arc};

use ryframe_api::handlers::captcha_handler::CaptchaStore;
use ryframe_common::{AppError, utils::create_storage_from_config};
use ryframe_config::AppConfig;
use ryframe_core::{
    AppContext, DataSourceManager, HotConfig, LoggedRepo, TokenBlacklist, create_redis_client,
    spawn_config_watcher,
};
use ryframe_db::{
    ConfigRepository, DbSpanLayer, DeptRepository, DictDataRepository, DictTypeRepository,
    JobLogRepository, JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository,
    OperLogRepository, PermissionRepository, PostRepository, RoleRepository, SqlLogLayer,
    UserRepository,
};
use ryframe_middleware::{
    RateLimiter,
    rate_limit::RateLimitState,
    telemetry::{TelemetryConfig, init_tracer_provider},
};
use ryframe_service::{
    AuthServiceImpl,
    system::{
        ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl, JobLogPersister,
        JobServiceImpl, LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl,
        OnlineUserServiceImpl, OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl,
        ProfileServiceImpl, RoleServiceImpl, UserServiceImpl,
    },
};
use ryframe_task::{
    ScheduledTask, TaskContext, TaskScheduler,
    builtin::{CleanLoginInfoTask, CleanOperLogTask, CleanTempFilesTask, DatabaseBackupTask},
};
use tracing_subscriber::{
    EnvFilter, Layer, filter::FilterFn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

/// 日志 Guard，保证滚动文件 writer 不被提前 Drop
struct LoggerGuard {
    _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// 初始化日志系统
///
/// - `output = "stdout"` → 控制台输出
/// - `output = "file"` → 滚动文件输出（每天滚动，保留 7 天）
/// - `format = "json"` → JSON 格式，否则 text 格式
/// - 同时初始化 OpenTelemetry 链路追踪（通过环境变量控制）
fn init_logger(
    config: &AppConfig,
) -> (
    LoggerGuard,
    Option<ryframe_middleware::telemetry::TelemetryGuard>,
) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logger.level));

    let is_json = config.logger.format == "json";
    let sql_log_level = config.database.sql_log_level;

    // 阻止 sqlx 查询事件到达 fmt 层（由 SqlLogLayer 单独格式化输出）
    let sqlx_filter = FilterFn::new(|meta| meta.target() != "sqlx::query");

    // 初始化链路追踪（在 subscriber 构建之前）
    let telemetry_config = TelemetryConfig {
        enabled: std::env::var("OTEL_ENABLED").unwrap_or_else(|_| "false".into()) == "true",
        endpoint: std::env::var("OTEL_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318/v1/traces".into()),
        service_name: std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "ryframe".into()),
        sample_rate: std::env::var("OTEL_SAMPLE_RATE")
            .unwrap_or_else(|_| "1.0".into())
            .parse()
            .unwrap_or(1.0),
    };
    let telemetry_guard = init_tracer_provider(&telemetry_config);
    let otel_layer = telemetry_guard.tracing_layer();

    // 构建 subscriber 的顺序很关键：
    // 1. fmt_layer（含 sqlx 过滤器）→ 2. SqlLogLayer → 3. otel(可选) → 4. env_filter
    // env_filter 放最后因为 EnvFilter: Layer<S> for all S: Subscriber，
    // 而 Filtered<FmtLayer, FilterFn, Registry> 只能 Layer<Registry>，
    // 无法 layer 到 Layered<EnvFilter, Registry> 上
    if config.logger.output == "file" {
        let file_appender = tracing_appender::rolling::daily("logs", "ryframe.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        // 构建 subscriber：先用 .boxed() 擦除 Filtered 类型
        let subscriber = if is_json {
            let fmt_layer = fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(sqlx_filter)
                .boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        } else {
            let fmt_layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(sqlx_filter)
                .boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        };

        if let Some(otel) = otel_layer {
            subscriber.with(otel).with(env_filter).init();
        } else {
            subscriber.with(env_filter).init();
        }

        (
            LoggerGuard {
                _worker: Some(guard),
            },
            Some(telemetry_guard),
        )
    } else {
        // 控制台输出
        let subscriber = if is_json {
            let fmt_layer = fmt::layer().json().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        } else {
            let fmt_layer = fmt::layer().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        };

        if let Some(otel) = otel_layer {
            subscriber.with(otel).with(env_filter).init();
        } else {
            subscriber.with(env_filter).init();
        }

        (LoggerGuard { _worker: None }, Some(telemetry_guard))
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // 1. 加载配置
    let config = AppConfig::load("config")?;
    tracing::info!(
        "配置加载完成, 环境: {}",
        std::env::var("APP_ENV").unwrap_or_else(|_| "dev".into())
    );

    // 1.1 创建热加载配置句柄（日志/限流/CORS/Redis 可热更新）
    let hot_config = HotConfig::new(config.clone());

    // 2. 初始化日志（内部同时初始化 OpenTelemetry 链路追踪）
    let (_logger_guard, _telemetry_guard) = init_logger(&config);
    tracing::info!(
        "日志系统初始化完成, 级别: {}, 输出: {}",
        config.logger.level,
        config.logger.output
    );

    // 2.1 启动进程指标采集后台任务（CPU/内存/FD/线程）
    ryframe_middleware::metrics::spawn_process_metrics_updater();

    // 3. 创建 DataSourceManager 并注册所有数据源
    let ds_manager = DataSourceManager::new();

    // 3a. 连接主库（connections[0]）
    let primary_config = &config.database.connections[0];
    let primary_db =
        ryframe_db::connection::connect_with_level(primary_config, config.database.sql_log_level)
            .await?;
    ds_manager.register("primary", primary_db.clone());
    tracing::info!("数据源 'primary' 连接成功: {}", primary_config.database);

    // 3b. 连接额外数据源（connections[1..]），命名为 db_1, db_2...
    let mut extra_dbs = Vec::with_capacity(config.database.connections.len().saturating_sub(1));
    for (i, conn_config) in config.database.connections.iter().enumerate().skip(1) {
        let name = format!("db_{}", i);
        match ryframe_db::connection::connect_with_level(conn_config, config.database.sql_log_level)
            .await
        {
            Ok(db) => {
                ds_manager.register(&name, db.clone());
                tracing::info!("数据源 '{}' 连接成功: {}", name, conn_config.database);
                extra_dbs.push(db);
            }
            Err(e) => {
                tracing::warn!(
                    "数据源 '{}' ({}) 连接失败: {}，跳过",
                    name,
                    conn_config.database,
                    e
                );
            }
        }
    }

    tracing::info!(
        "DataSourceManager 初始化完成, 共 {} 个数据源: {:?}",
        ds_manager.len(),
        ds_manager.names()
    );

    // 设为全局单例，业务代码可通过 ryframe_core::current_db() 直接访问
    ds_manager.clone().set_global();

    // 3.x 健康检查 primary
    ryframe_db::connection::ping(&primary_db).await?;

    // 3.y 检查所有必需表是否存在
    if let Err(missing) = ryframe_db::connection::check_tables(&primary_db).await {
        eprintln!("\n========================================");
        eprintln!("  数据库表缺失！请先执行建表 SQL：");
        eprintln!("    mysql -u root -p ryframe_config < sql/ryframe_config.sql");
        eprintln!("========================================");
        eprintln!("  缺失的表 ({} 张):", missing.len());
        for table in &missing {
            eprintln!("    - {}", table);
        }
        eprintln!("========================================\n");
        return Err(AppError::Internal(format!(
            "缺少 {} 张必需的数据表，请先执行 sql/ryframe_config.sql 初始化数据库",
            missing.len()
        )));
    }
    tracing::info!("数据库表检查通过 ({} 张表全部存在)", 19);

    // 5. 创建 Application Context
    let context = AppContext::new(config.clone());

    // 6. 创建 Repository 实例（DataSourceManager 通过全局单例访问）

    let oper_log_repo = LoggedRepo::new(OperLogRepository);
    let login_info_repo = LoggedRepo::new(LoginInfoRepository);

    let config_arc = Arc::new(config.clone());

    // 7. 创建 Service（构造注入 Repository + Config）
    let auth_service = Arc::new(AuthServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        config: Arc::new(config.clone()),
    });

    let user_service = Arc::new(UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    });

    let role_service = Arc::new(RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    });

    let permission_service = Arc::new(PermissionServiceImpl {
        perm_repo: LoggedRepo::new(PermissionRepository),
    });

    // 7.w 创建 Redis 客户端（提前初始化，供菜单/部门/字典/配置服务使用）
    let redis_client = create_redis_client(&config.redis).await;
    if redis_client.is_some() {
        tracing::info!(
            "Redis 已启用，验证码/在线用户/限流器/菜单树/部门树/字典缓存将使用 Redis 存储"
        );
    } else {
        tracing::info!("Redis 未配置或不可用，使用内存模式");
    }

    // 7.w2 创建 Token 黑名单（Redis 或内存模式）
    let token_blacklist = TokenBlacklist::new(redis_client.clone());
    if redis_client.is_none() {
        token_blacklist.spawn_gc(); // 内存模式需要后台 GC
    }

    let menu_service = Arc::new(MenuServiceImpl {
        menu_repo: LoggedRepo::new(MenuRepository),
        redis: redis_client.clone(),
    });

    let dept_service = Arc::new(DeptServiceImpl {
        dept_repo: LoggedRepo::new(DeptRepository),
        redis: redis_client.clone(),
    });

    let post_service = Arc::new(PostServiceImpl {
        post_repo: LoggedRepo::new(PostRepository),
    });

    let config_service = Arc::new(ConfigServiceImpl {
        config_repo: LoggedRepo::new(ConfigRepository),
        redis: redis_client.clone(),
    });

    let dict_service = Arc::new(DictServiceImpl {
        dict_type_repo: LoggedRepo::new(DictTypeRepository),
        dict_data_repo: LoggedRepo::new(DictDataRepository),
        redis: redis_client.clone(),
    });

    let notice_service = Arc::new(NoticeServiceImpl {
        notice_repo: LoggedRepo::new(NoticeRepository),
    });

    let oper_log_service = Arc::new(OperLogServiceImpl { oper_log_repo });

    let login_info_service = Arc::new(LoginInfoServiceImpl { login_info_repo });

    // 7.x 创建 TaskScheduler
    let task_ctx = TaskContext {
        db: Arc::new(primary_db.clone()),
    };
    let job_log_repo = LoggedRepo::new(JobLogRepository);

    let mut scheduler = TaskScheduler::new(task_ctx.clone());
    scheduler.set_persister(Arc::new(JobLogPersister {
        job_log_repo: job_log_repo.clone(),
        db: Arc::new(primary_db.clone()),
    }));
    let scheduler = Arc::new(scheduler);

    // 7.y 创建 JobService（注入 scheduler）并初始化内置任务
    let job_service = Arc::new(JobServiceImpl {
        job_repo: LoggedRepo::new(JobRepository),
        job_log_repo,
        scheduler: scheduler.clone(),
    });

    // 从数据库加载配置并注册内置任务
    // - DB 有记录：使用 DB 中配置的 cron 和 status
    // - DB 无记录：插入默认配置，使用任务代码中的默认 cron
    let builtin_tasks: Vec<Arc<dyn ScheduledTask>> = vec![
        Arc::new(CleanOperLogTask),
        Arc::new(CleanLoginInfoTask),
        Arc::new(CleanTempFilesTask),
        Arc::new(DatabaseBackupTask),
    ];
    job_service
        .init_builtin_tasks(&primary_db, &builtin_tasks)
        .await?;
    tracing::info!("已注册 {} 个内置定时任务", scheduler.list().await.len());

    // 7.z 创建 GeneratorService
    let workspace_root = std::env::current_dir()
        .map_err(|e| AppError::Internal(format!("无法获取 workspace root: {}", e)))?;
    let generator_service = Arc::new(GeneratorServiceImpl { workspace_root });

    // 7.aa 创建 ProfileService
    let profile_service = Arc::new(ProfileServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
    });

    // 7.cc 创建 OnlineUserService（Redis 或内存模式）
    let online_user_service = if let Some(ref redis) = redis_client {
        Arc::new(OnlineUserServiceImpl::new_redis(redis.clone()))
    } else {
        Arc::new(OnlineUserServiceImpl::new_in_memory())
    };

    // 启动时清理残留的旧在线用户会话
    online_user_service.clear_all_on_startup().await;

    // 7.dd 创建 CaptchaStore（Redis 或内存模式）
    let captcha_store = if let Some(ref redis) = redis_client {
        CaptchaStore::new_redis(redis.clone(), 300)
    } else {
        let store = CaptchaStore::new_in_memory(300);
        store.spawn_gc(); // 内存模式需要后台 GC
        store
    };

    // 8. 限流器（Redis 或内存模式）
    let limiter = if let Some(ref redis) = redis_client {
        let window = if config.rate_limit.window_secs > 0 {
            config.rate_limit.window_secs
        } else {
            60 // 默认 60 秒固定窗口
        };
        Arc::new(RateLimiter::new_redis(
            redis.clone(),
            config.rate_limit.capacity,
            window,
        ))
    } else {
        let l = Arc::new(RateLimiter::new_in_memory(
            config.rate_limit.capacity,
            config.rate_limit.refill_per_sec,
        ));
        l.spawn_gc();
        l
    };

    // 8.1 创建限流状态（用于 per-user / per-api 中间件）
    let rate_limit_state = RateLimitState {
        limiter: limiter.clone(),
        config: Arc::new(config.rate_limit.clone()),
    };

    // 8.1 创建对象存储（根据配置自动选择本地/MinIO/S3，MinIO 初始化失败时降级为本地）
    let storage_config = &config.object_storage;
    let object_storage: Arc<dyn ryframe_common::utils::ObjectStorage> =
        Arc::from(create_storage_from_config(
            match storage_config.backend {
                ryframe_config::StorageBackend::Local => "local",
                ryframe_config::StorageBackend::Minio => "minio",
                ryframe_config::StorageBackend::S3 => "s3",
            },
            &storage_config.local_base_dir,
            &storage_config.public_base_url,
            &storage_config.endpoint,
            &storage_config.access_key,
            &storage_config.secret_key,
            storage_config.use_ssl,
        ));
    tracing::info!(
        "对象存储初始化完成, 后端: {}, 本地目录: {}",
        match storage_config.backend {
            ryframe_config::StorageBackend::Local => "local",
            ryframe_config::StorageBackend::Minio => "minio",
            ryframe_config::StorageBackend::S3 => "s3",
        },
        storage_config.local_base_dir
    );

    // 9. 获取监听地址
    let addr = format!("{}:{}", config.app.host, config.app.port);

    // 10. 创建 AppState + 构建 Router
    let state = ryframe_api::AppState {
        datasource_manager: ds_manager.clone(),
        db: primary_db.clone(),
        config: config_arc,
        context,
        auth_service,
        user_service,
        role_service,
        permission_service,
        menu_service,
        dept_service,
        post_service,
        config_service,
        dict_service,
        notice_service,
        oper_log_service,
        login_info_service,
        job_service,
        generator_service,
        profile_service,
        online_user_service,
        captcha_store,
        scheduler: scheduler.clone(),
        monitor_db: primary_db.clone(),
        redis: redis_client.clone(),
        token_blacklist,
        replica_dbs: extra_dbs,
        rate_limiter: limiter.clone(),
        object_storage,
    };
    let router = app::build_app(state, limiter, rate_limit_state, &config.cors);

    // 11. 启动 scheduler 后台
    let scheduler_for_shutdown = scheduler.clone();
    scheduler.spawn();
    tracing::info!("TaskScheduler 已启动");

    // 12. 启动 HTTP 服务
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| AppError::Internal(format!("绑定地址 {} 失败: {}", addr, e)))?;

    tracing::info!("服务启动: http://{}", addr);

    // 13. 启动配置文件热加载（每 5 秒检查一次）
    spawn_config_watcher(
        hot_config,
        "config".to_string(),
        Some(Arc::new(move |new_config: &AppConfig| {
            // 配置变更时提示（日志级别变更需重启 tracing subscriber 才能完整生效）
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
        result = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown_signal()) => {
            result.map_err(|e| AppError::Internal(format!("服务异常退出: {}", e)))?;
        }
    }

    // 优雅关闭：停止定时任务调度器
    scheduler_for_shutdown.shutdown();
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

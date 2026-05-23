mod app;

use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::AppContext;
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository,
    JobLogRepository, JobRepository, LoginInfoRepository, MenuRepository,
    NoticeRepository, OperLogRepository, PermissionRepository, PostRepository,
    RoleRepository, UserRepository,
};
use ryframe_service::system::{
    ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl,
    JobServiceImpl, LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl,
    OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, RoleServiceImpl,
    UserServiceImpl, ProfileServiceImpl, OnlineUserServiceImpl,
};
use ryframe_service::AuthServiceImpl;
use ryframe_task::{ScheduledTask, TaskContext, TaskScheduler};
use ryframe_task::builtin::{CleanLoginInfoTask, CleanOperLogTask, CleanTempFilesTask, DatabaseBackupTask};
use ryframe_middleware::RateLimiter;
use ryframe_api::handlers::captcha_handler::CaptchaStore;
use ryframe_core::create_redis_client;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// 日志 Guard，保证滚动文件 writer 不被提前 Drop
struct LoggerGuard {
    _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// 初始化日志系统
///
/// - `output = "stdout"` → 控制台输出
/// - `output = "file"` → 滚动文件输出（每天滚动，保留 7 天）
/// - `format = "json"` → JSON 格式，否则 text 格式
fn init_logger(config: &AppConfig) -> LoggerGuard {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.logger.level));

    let is_json = config.logger.format == "json";

    if config.logger.output == "file" {
        // 滚动文件输出：logs/ryframe-yyyy-MM-dd.log
        let file_appender = tracing_appender::rolling::daily("logs", "ryframe.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        if is_json {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().json().with_writer(non_blocking.clone()).with_ansi(false))
                .init();
        } else {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
                .init();
        }

        LoggerGuard { _worker: Some(guard) }
    } else {
        // 控制台输出
        if is_json {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().json())
                .init();
        } else {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer())
                .init();
        }

        LoggerGuard { _worker: None }
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

    // 2. 初始化日志（根据配置支持 stdout / 滚动文件输出）
    let _guard = init_logger(&config);
    tracing::info!("日志系统初始化完成, 级别: {}, 输出: {}", config.logger.level, config.logger.output);

    // 3. 连接数据库
    let db = ryframe_db::connection::connect(&config.database.primary).await?;
    tracing::info!("数据库连接成功");

    // 3.a 连接从库（读写分离）
    let replica_dbs = {
        let mut dbs = Vec::with_capacity(config.database.replicas.len());
        for replica_config in &config.database.replicas {
            match ryframe_db::connection::connect(replica_config).await {
                Ok(replica_db) => {
                    tracing::info!("从库连接成功: {}", replica_config.database);
                    dbs.push(replica_db);
                }
                Err(e) => {
                    tracing::warn!("从库连接失败 ({}): {}，跳过", replica_config.database, e);
                }
            }
        }
        dbs
    };

    if replica_dbs.is_empty() && !config.database.replicas.is_empty() {
        tracing::warn!("所有从库连接均失败，仅使用主库");
    } else if !replica_dbs.is_empty() {
        tracing::info!("已连接 {} 个从库", replica_dbs.len());
    }

    // 3.x 健康检查
    ryframe_db::connection::ping(&db).await?;

    // 3.y 检查所有必需表是否存在
    if let Err(missing) = ryframe_db::connection::check_tables(&db).await {
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
    tracing::info!("数据库表检查通过 ({} 张表全部存在)", 17);

    // 5. 创建 Application Context
    let context = AppContext::new(config.clone());

    // 6. 创建 Repository 实例
    let user_repo = UserRepository;
    let role_repo = RoleRepository;
    let perm_repo = PermissionRepository;
    let oper_log_repo = OperLogRepository;
    let login_info_repo = LoginInfoRepository;

    let config_arc = Arc::new(config.clone());

    // 7. 创建 Service（构造注入 Repository + Config）
    let auth_service = Arc::new(AuthServiceImpl {
        user_repo,
        role_repo,
        perm_repo,
        config: Arc::new(config.clone()),
    });

    let user_service = Arc::new(UserServiceImpl {
        user_repo: UserRepository,
        role_repo: RoleRepository,
        dept_repo: DeptRepository,
    });

    let role_service = Arc::new(RoleServiceImpl {
        role_repo: RoleRepository,
        perm_repo: PermissionRepository,
        menu_repo: MenuRepository,
    });

    let permission_service = Arc::new(PermissionServiceImpl {
        perm_repo: PermissionRepository,
    });

    let menu_service = Arc::new(MenuServiceImpl {
        menu_repo: MenuRepository,
    });

    let dept_service = Arc::new(DeptServiceImpl {
        dept_repo: DeptRepository,
    });

    let post_service = Arc::new(PostServiceImpl {
        post_repo: PostRepository,
    });

    // 7.w 创建 Redis 客户端（提前初始化，供字典/配置服务使用）
    let redis_client = create_redis_client(&config.redis).await;
    if redis_client.is_some() {
        tracing::info!("Redis 已启用，验证码/在线用户/限流器/字典缓存将使用 Redis 存储");
    } else {
        tracing::info!("Redis 未配置或不可用，使用内存模式");
    }

    let config_service = Arc::new(ConfigServiceImpl {
        config_repo: ConfigRepository,
        redis: redis_client.clone(),
    });

    let dict_service = Arc::new(DictServiceImpl {
        dict_type_repo: DictTypeRepository,
        dict_data_repo: DictDataRepository,
        redis: redis_client.clone(),
    });

    let notice_service = Arc::new(NoticeServiceImpl {
        notice_repo: NoticeRepository,
    });

    let oper_log_service = Arc::new(OperLogServiceImpl {
        oper_log_repo,
    });

    let login_info_service = Arc::new(LoginInfoServiceImpl {
        login_info_repo,
    });

    // 7.x 创建 TaskScheduler
    let task_ctx = TaskContext {
        db: Arc::new(db.clone()),
    };
    let scheduler = Arc::new(TaskScheduler::new(task_ctx.clone()));

    // 7.y 创建 JobService（注入 scheduler）并初始化内置任务
    let job_service = Arc::new(JobServiceImpl {
        job_repo: JobRepository,
        job_log_repo: JobLogRepository,
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
    job_service.init_builtin_tasks(&db, &builtin_tasks).await?;
    tracing::info!("已注册 {} 个内置定时任务", scheduler.list().await.len());

    // 7.z 创建 GeneratorService
    let workspace_root = std::env::current_dir()
        .map_err(|e| AppError::Internal(format!("无法获取 workspace root: {}", e)))?;
    let generator_service = Arc::new(GeneratorServiceImpl { workspace_root });

    // 7.aa 创建 ProfileService
    let profile_service = Arc::new(ProfileServiceImpl {
        user_repo: UserRepository,
        role_repo: RoleRepository,
        perm_repo: PermissionRepository,
    });

    // 7.cc 创建 OnlineUserService（Redis 或内存模式）
    let online_user_service = if let Some(ref redis) = redis_client {
        Arc::new(OnlineUserServiceImpl::new_redis(redis.clone()))
    } else {
        Arc::new(OnlineUserServiceImpl::new_in_memory())
    };

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
        Arc::new(RateLimiter::new_redis(
            redis.clone(),
            config.rate_limit.capacity,
            60, // 60 秒固定窗口
        ))
    } else {
        let l = Arc::new(RateLimiter::new_in_memory(
            config.rate_limit.capacity,
            config.rate_limit.refill_per_sec,
        ));
        l.spawn_gc();
        l
    };

    // 9. 获取监听地址
    let addr = format!("{}:{}", config.app.host, config.app.port);

    // 10. 创建 AppState + 构建 Router
    let state = ryframe_api::AppState {
        db: db.clone(),
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
        monitor_db: db.clone(),
        redis: redis_client.clone(),
        replica_dbs,
    };
    let router = app::build_app(state, limiter, &config.cors);

    // 11. 启动 scheduler 后台
    let scheduler_for_shutdown = scheduler.clone();
    scheduler.spawn();
    tracing::info!("TaskScheduler 已启动");

    // 12. 启动 HTTP 服务
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| AppError::Internal(format!("绑定地址 {} 失败: {}", addr, e)))?;

    tracing::info!("服务启动: http://{}", addr);

    // 使用 tokio::select 同时等待服务和停机信号
    tokio::select! {
        result = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()) => {
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
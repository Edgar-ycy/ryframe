use std::sync::Arc;

use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::{LoggedRepo, RedisClient};
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, JobLogRepository,
    JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository,
    PermissionRepository, PostRepository, RoleRepository, UserRepository,
};
use ryframe_service::{
    AuthServiceImpl,
    system::{
        CaptchaStore, ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl,
        JobLogPersister, JobServiceImpl, LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl,
        OnlineUserServiceImpl, OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl,
        ProfileServiceImpl, RoleServiceImpl, UserServiceImpl,
    },
};
use ryframe_task::{
    ScheduledTask, TaskContext, TaskScheduler,
    builtin::{CleanLoginInfoTask, CleanOperLogTask, CleanTempFilesTask, DatabaseBackupTask},
};
use sea_orm::DatabaseConnection;

/// 所有业务 Service 实例的容器
pub struct Services {
    pub auth_service: Arc<AuthServiceImpl>,
    pub user_service: Arc<UserServiceImpl>,
    pub role_service: Arc<RoleServiceImpl>,
    pub permission_service: Arc<PermissionServiceImpl>,
    pub menu_service: Arc<MenuServiceImpl>,
    pub dept_service: Arc<DeptServiceImpl>,
    pub post_service: Arc<PostServiceImpl>,
    pub config_service: Arc<ConfigServiceImpl>,
    pub dict_service: Arc<DictServiceImpl>,
    pub notice_service: Arc<NoticeServiceImpl>,
    pub oper_log_service: Arc<OperLogServiceImpl>,
    pub login_info_service: Arc<LoginInfoServiceImpl>,
    pub job_service: Arc<JobServiceImpl>,
    pub generator_service: Arc<GeneratorServiceImpl>,
    pub profile_service: Arc<ProfileServiceImpl>,
    pub online_user_service: Arc<OnlineUserServiceImpl>,
    pub captcha_store: CaptchaStore,
    pub scheduler: Arc<TaskScheduler>,
}

/// 构造所有 Service 实例
///
/// 依赖注入顺序：Repository → Redis → Service。
pub async fn build_all(
    config: &AppConfig,
    redis_client: &Option<RedisClient>,
    primary_db: &DatabaseConnection,
) -> Result<Services, AppError> {
    let oper_log_repo = LoggedRepo::new(OperLogRepository);
    let login_info_repo = LoggedRepo::new(LoginInfoRepository);

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

    let auth_service = Arc::new(AuthServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        config: Arc::new(config.clone()),
        redis: redis_client.clone(),
    });

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

    // -- TaskScheduler + JobService --
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

    let job_service = Arc::new(JobServiceImpl {
        job_repo: LoggedRepo::new(JobRepository),
        job_log_repo,
        scheduler: scheduler.clone(),
    });

    // 从数据库加载配置并注册内置任务
    let builtin_tasks: Vec<Arc<dyn ScheduledTask>> = vec![
        Arc::new(CleanOperLogTask),
        Arc::new(CleanLoginInfoTask),
        Arc::new(CleanTempFilesTask),
        Arc::new(DatabaseBackupTask),
    ];
    job_service
        .init_builtin_tasks(primary_db, &builtin_tasks)
        .await?;
    tracing::info!("已注册 {} 个内置定时任务", scheduler.list().await.len());

    // -- GeneratorService --
    let workspace_root = std::env::current_dir()
        .map_err(|e| AppError::Internal(format!("无法获取 workspace root: {}", e)))?;
    let generator_service = Arc::new(GeneratorServiceImpl { workspace_root });

    // -- ProfileService --
    let profile_service = Arc::new(ProfileServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
    });

    // -- OnlineUserService（Redis 或内存模式） --
    let online_user_service: Arc<OnlineUserServiceImpl> = if let Some(redis) = redis_client {
        Arc::new(OnlineUserServiceImpl::new_redis(redis.clone()))
    } else {
        Arc::new(OnlineUserServiceImpl::new_in_memory())
    };
    // 启动时清理残留的旧在线用户会话
    online_user_service.clear_all_on_startup().await;

    // -- CaptchaStore（Redis 或内存模式） --
    let captcha_store = if let Some(redis) = redis_client {
        CaptchaStore::new_redis(redis.clone(), 300)
    } else {
        let store = CaptchaStore::new_in_memory(300);
        store.spawn_gc(); // 内存模式需要后台 GC
        store
    };

    Ok(Services {
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
        scheduler,
    })
}

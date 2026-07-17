use std::sync::Arc;

use ryframe_api::AppServices;
use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::RedisClient;
use ryframe_db::DatabaseCluster;
use ryframe_service::{
    AuthService,
    system::{
        CaptchaStore, ConfigService, DeptService, DictService, FileService, GeneratorService,
        LoginInfoService, MenuService, NoticeService, OnlineUserService, OperLogService,
        PermissionService, PostService, ProfileService, RoleService, TenantService, UserService,
    },
};
use ryframe_storage::ObjectStorage;

/// 构造所有 Service 实例
///
/// 依赖注入顺序：Repository → Redis → Service。
pub async fn build_all(
    database: &DatabaseCluster,
    config: &AppConfig,
    redis_client: &Option<RedisClient>,
    object_storage: Arc<dyn ObjectStorage>,
) -> Result<AppServices, AppError> {
    let user = Arc::new(UserService::new(database.clone(), redis_client.clone()));
    let role = Arc::new(RoleService::new(database.clone(), redis_client.clone()));
    let tenant = Arc::new(TenantService::new(database.clone()));
    let permission = Arc::new(PermissionService::new(
        database.clone(),
        redis_client.clone(),
    ));
    let auth = Arc::new(AuthService::new(
        database.clone(),
        Arc::new(config.clone()),
        redis_client.clone(),
    ));
    let menu = Arc::new(MenuService::new(database.clone(), redis_client.clone()));
    // 启动时清除菜单树缓存，确保迁移新增的菜单项能立即显示
    menu.invalidate_all_menu_caches().await;

    let dept = Arc::new(DeptService::new(database.clone(), redis_client.clone()));
    let post = Arc::new(PostService::new(database.clone()));
    let config_service = Arc::new(ConfigService::new(database.clone(), redis_client.clone()));

    let dict = Arc::new(DictService::new(database.clone(), redis_client.clone()));
    let notice = Arc::new(NoticeService::new(database.clone()));
    let oper_log = Arc::new(OperLogService::new(database.clone()));
    let login_info = Arc::new(LoginInfoService::new(database.clone()));

    let project_root = std::env::current_dir()
        .map_err(|e| AppError::Internal(format!("无法获取项目根目录: {}", e)))?;
    if config.generator.data_source != "primary" {
        database
            .source(&config.generator.data_source)
            .ok_or_else(|| {
                AppError::Config(format!(
                    "代码生成器数据源未连接: {}",
                    config.generator.data_source
                ))
            })?;
    }
    let generator = Arc::new(GeneratorService::new(
        database.clone(),
        config.generator.data_source.clone(),
        project_root,
    ));

    let profile = Arc::new(ProfileService::new(database.clone()));
    let file = Arc::new(FileService::new(database.clone(), object_storage));

    let online_user: Arc<OnlineUserService> = if let Some(redis) = redis_client {
        Arc::new(OnlineUserService::new_redis(redis.clone()))
    } else {
        Arc::new(OnlineUserService::new_in_memory())
    };
    // 启动时清理残留的旧在线用户会话
    online_user.clear_all_on_startup().await;

    let captcha = if let Some(redis) = redis_client {
        CaptchaStore::new_redis(redis.clone(), 300)
    } else {
        let store = CaptchaStore::new_in_memory(300);
        store.spawn_gc(); // 内存模式需要后台 GC
        store
    };

    Ok(AppServices {
        auth,
        user,
        role,
        tenant,
        permission,
        menu,
        dept,
        post,
        config: config_service,
        dict,
        notice,
        oper_log,
        login_info,
        generator,
        profile,
        file,
        online_user,
        captcha,
    })
}

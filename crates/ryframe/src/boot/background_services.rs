use std::sync::Arc;

use ryframe_adapters::RedisClient;
use ryframe_application::{
    AuthorizationCache, JobQueue, JobScheduleService,
    ports::{auth::IdentityAuthorizationReadPort, files::ArtifactStore},
    system::{
        content::{ConfigService, DictCacheStore, DictService, FileService, PostExportService},
        identity::{RoleService, UserImportService, UserService},
        operations::{
            DataRetentionService, ExportPersistencePorts, ExportResourceServices, ExportService,
            LoginInfoService, MessageService, OperLogService, OverviewService,
        },
        platform::{ProductService, TenantConfigTransferService, TenantDataMigrationService},
    },
};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, AppResult};
use ryframe_tenant_db::TenantDatabaseRouter;

use super::application_policy::ApplicationPolicies;

pub struct BackgroundServiceInfrastructure {
    pub redis_client: Option<RedisClient>,
    pub object_storage: Arc<dyn ArtifactStore>,
    pub dict_cache: Option<Arc<dyn DictCacheStore>>,
    pub starts_background_tasks: bool,
}

/// API 与独立 Worker 共用的后台业务服务。
pub struct BackgroundServices {
    pub authorization_cache: AuthorizationCache,
    pub identity_read: Arc<dyn IdentityAuthorizationReadPort>,
    pub user: Arc<UserService>,
    pub product: Arc<ProductService>,
    pub role: Arc<RoleService>,
    pub config: Arc<ConfigService>,
    pub dict: Arc<DictService>,
    pub oper_log: Arc<OperLogService>,
    pub login_info: Arc<LoginInfoService>,
    pub file: Arc<FileService>,
    pub job_queue: Arc<JobQueue>,
    pub job_schedules: Option<Arc<JobScheduleService>>,
    pub message: Arc<MessageService>,
    pub export: Arc<ExportService>,
    pub data_retention: Arc<DataRetentionService>,
    pub user_import: Arc<UserImportService>,
    pub tenant_config_transfer: Arc<TenantConfigTransferService>,
    pub tenant_data_migration: Arc<TenantDataMigrationService>,
    pub overview: Arc<OverviewService>,
}

struct IdentityServices {
    authorization_cache: AuthorizationCache,
    identity_read: Arc<dyn IdentityAuthorizationReadPort>,
    user: Arc<UserService>,
    product: Arc<ProductService>,
    role: Arc<RoleService>,
}

struct ContentServices {
    config: Arc<ConfigService>,
    dict: Arc<DictService>,
    oper_log: Arc<OperLogService>,
    login_info: Arc<LoginInfoService>,
    file: Arc<FileService>,
    job_queue: Arc<JobQueue>,
}

struct ExtendedServices {
    job_schedules: Option<Arc<JobScheduleService>>,
    message: Arc<MessageService>,
    export: Arc<ExportService>,
    data_retention: Arc<DataRetentionService>,
    user_import: Arc<UserImportService>,
    tenant_config_transfer: Arc<TenantConfigTransferService>,
    tenant_data_migration: Arc<TenantDataMigrationService>,
    overview: Arc<OverviewService>,
}

/// 构造 API 与 Worker 必须保持一致的后台服务依赖图。
pub fn build(
    database: &ControlDatabaseCluster,
    tenant_data: Arc<TenantDatabaseRouter>,
    policies: &ApplicationPolicies,
    infrastructure: BackgroundServiceInfrastructure,
) -> AppResult<BackgroundServices> {
    let BackgroundServiceInfrastructure {
        redis_client,
        object_storage,
        dict_cache,
        starts_background_tasks,
    } = infrastructure;
    let identity = build_identity_services(database, policies, redis_client.as_ref());
    let content = build_content_services(
        database,
        &identity.authorization_cache,
        object_storage.clone(),
        dict_cache,
        redis_client.as_ref(),
        starts_background_tasks,
    );
    let extended = build_extended_services(
        database,
        tenant_data,
        policies,
        object_storage,
        &identity,
        &content,
    )?;
    Ok(BackgroundServices {
        authorization_cache: identity.authorization_cache,
        identity_read: identity.identity_read,
        user: identity.user,
        product: identity.product,
        role: identity.role,
        config: content.config,
        dict: content.dict,
        oper_log: content.oper_log,
        login_info: content.login_info,
        file: content.file,
        job_queue: content.job_queue,
        job_schedules: extended.job_schedules,
        message: extended.message,
        export: extended.export,
        data_retention: extended.data_retention,
        user_import: extended.user_import,
        tenant_config_transfer: extended.tenant_config_transfer,
        tenant_data_migration: extended.tenant_data_migration,
        overview: extended.overview,
    })
}

fn build_identity_services(
    database: &ControlDatabaseCluster,
    policies: &ApplicationPolicies,
    redis_client: Option<&RedisClient>,
) -> IdentityServices {
    let authorization_cache =
        super::authorization_cache::cache(redis_client.cloned(), policies.cache);
    let identity_read = ryframe_db::application_ports::auth::identity(database.clone());
    let user = Arc::new(UserService::new(
        authorization_cache.clone(),
        Arc::clone(&identity_read),
        ryframe_db::application_ports::users::query(database.clone()),
        ryframe_db::application_ports::users::write(database.clone(), authorization_cache.clone()),
        ryframe_db::application_ports::auth::password_reset(
            database.clone(),
            authorization_cache.clone(),
        ),
    ));
    let product = Arc::new(ProductService::new(
        ryframe_db::application_ports::product::read(database.clone()),
        ryframe_db::application_ports::product::write(database.clone()),
        authorization_cache.clone(),
    ));
    let role = Arc::new(RoleService::new(
        authorization_cache.clone(),
        ryframe_db::application_ports::system::role_read(database.clone()),
        ryframe_db::application_ports::system::role_write(
            database.clone(),
            authorization_cache.clone(),
            Arc::clone(&product),
        ),
    ));
    IdentityServices {
        authorization_cache,
        identity_read,
        user,
        product,
        role,
    }
}

fn build_content_services(
    database: &ControlDatabaseCluster,
    authorization_cache: &AuthorizationCache,
    object_storage: Arc<dyn ArtifactStore>,
    dict_cache: Option<Arc<dyn DictCacheStore>>,
    redis_client: Option<&RedisClient>,
    starts_background_tasks: bool,
) -> ContentServices {
    let config = Arc::new(ConfigService::new(
        ryframe_db::application_ports::system::config(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache.clone(),
    ));
    let dict = Arc::new(DictService::new(
        ryframe_db::application_ports::system::dict(database.clone()),
        dict_cache,
    ));
    let oper_log = Arc::new(OperLogService::new(
        ryframe_db::application_ports::system::oper_log(database.clone()),
    ));
    let login_info = Arc::new(LoginInfoService::new(
        ryframe_db::application_ports::system::login_info(database.clone()),
    ));
    let file = Arc::new(FileService::new(
        ryframe_db::application_ports::files::cleanup(database.clone()),
        ryframe_db::application_ports::files::download(database.clone()),
        ryframe_db::application_ports::files::upload(database.clone()),
        object_storage.clone(),
        super::file_content::processor(),
    ));
    if starts_background_tasks {
        file.spawn_upload_janitor();
    }
    let job_queue = Arc::new(
        JobQueue::new(ryframe_db::application_ports::jobs::queue(database.clone()))
            .with_wakeup_transport(super::jobs::job_wakeup_transport(redis_client)),
    );
    ContentServices {
        config,
        dict,
        oper_log,
        login_info,
        file,
        job_queue,
    }
}

fn build_extended_services(
    database: &ControlDatabaseCluster,
    tenant_data: Arc<TenantDatabaseRouter>,
    policies: &ApplicationPolicies,
    object_storage: Arc<dyn ArtifactStore>,
    identity: &IdentityServices,
    content: &ContentServices,
) -> AppResult<ExtendedServices> {
    let authorization_cache = &identity.authorization_cache;
    let user = &identity.user;
    let product = &identity.product;
    let file = &content.file;
    let job_queue = &content.job_queue;
    let tenant_data_migration = Arc::new(TenantDataMigrationService::new(
        ryframe_tenant_db::application_ports::tenant_data::tracking(database.clone()),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
        Arc::<TenantDatabaseRouter>::clone(&tenant_data),
        job_queue.clone(),
        authorization_cache.clone(),
    ));
    let data_retention = Arc::new(DataRetentionService::new(
        ryframe_db::application_ports::tenant_config::retention(database.clone()),
        ryframe_db::application_ports::retention::cleanup(database.clone()),
        ryframe_db::application_ports::retention::run(database.clone()),
        job_queue.clone(),
        file.clone(),
        policies.retention,
    ));
    let user_import = Arc::new(UserImportService::new(
        job_queue.clone(),
        user.clone(),
        file.clone(),
        super::spreadsheet::document_processor(),
        ryframe_db::application_ports::users::import(database.clone()),
        policies.user_import,
    ));
    let tenant_config_transfer = Arc::new(TenantConfigTransferService::new(
        ryframe_application::system::platform::TenantConfigTransferDependencies {
            persistence: ryframe_db::application_ports::tenant_config::transfer(database.clone()),
            queue: job_queue.clone(),
            user: user.clone(),
            file: file.clone(),
            product: product.clone(),
            authorization_cache: authorization_cache.clone(),
            archive: super::tenant_config_archive::codec(),
        },
        ryframe_application::system::platform::TenantConfigTransferSettings {
            target_catalog: super::access_catalog::tenant_config_target_catalog()?,
            config: policies.tenant_config_transfer,
        },
    ));
    let overview = Arc::new(OverviewService::new(
        ryframe_db::application_ports::system::overview(database.clone()),
        job_queue.clone(),
        policies.job_runtime,
    ));
    let job_schedules = build_schedules(database, job_queue, policies)?;
    let message = Arc::new(MessageService::new(
        ryframe_db::application_ports::system::message(database.clone()),
        job_queue.clone(),
        policies.messaging,
    ));
    let export = build_export_service(database, object_storage, identity, content, policies);

    Ok(ExtendedServices {
        job_schedules,
        message,
        export,
        data_retention,
        user_import,
        tenant_config_transfer,
        tenant_data_migration,
        overview,
    })
}

fn build_export_service(
    database: &ControlDatabaseCluster,
    object_storage: Arc<dyn ArtifactStore>,
    identity: &IdentityServices,
    content: &ContentServices,
    policies: &ApplicationPolicies,
) -> Arc<ExportService> {
    let persistence = ExportPersistencePorts::new(
        ryframe_db::application_ports::export::artifact(database.clone()),
        ryframe_db::application_ports::export::cleanup(database.clone()),
        ryframe_db::application_ports::export::deletion(database.clone()),
        ryframe_db::application_ports::export::execution(database.clone()),
        ryframe_db::application_ports::export::request(database.clone()),
        ryframe_db::application_ports::export::requester(database.clone()),
    );
    let resources = ExportResourceServices {
        users: Arc::clone(&identity.user),
        roles: Arc::clone(&identity.role),
        posts: Arc::new(PostExportService::new(
            ryframe_db::application_ports::export::post(database.clone()),
        )),
        configs: Arc::clone(&content.config),
        dicts: Arc::clone(&content.dict),
        oper_logs: Arc::clone(&content.oper_log),
        login_infos: Arc::clone(&content.login_info),
    };
    Arc::new(
        ExportService::new(
            persistence,
            resources,
            object_storage,
            super::spreadsheet::writer_factory(),
            policies.export,
        )
        .with_job_queue(content.job_queue.clone()),
    )
}

fn build_schedules(
    database: &ControlDatabaseCluster,
    queue: &Arc<JobQueue>,
    policies: &ApplicationPolicies,
) -> Result<Option<Arc<JobScheduleService>>, AppError> {
    if !policies.job_schedule.enabled {
        return Ok(None);
    }
    let targets = super::jobs::build_schedule_targets(policies.messaging.enabled())?;
    Ok(Some(Arc::new(
        JobScheduleService::new(
            ryframe_db::application_ports::jobs::schedule(database.clone()),
            queue.clone(),
            super::jobs::execution_tenant_scope(policies.multi_tenancy),
            targets,
            policies.job_schedule,
        )
        .with_metrics_observer(super::jobs::build_schedule_metrics_observer()),
    )))
}

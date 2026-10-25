use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use axum::{Router, middleware::from_fn_with_state};
use ryframe::{boot, boot::app_state::AppStateAssembly};
use ryframe_adapters::{TokenBlacklist, i18n::LocalizerLoader, monitor::ServerInfoSampler};
use ryframe_api::{
    AppState, auth_middleware::auth_middleware, handlers::user_handler::user_router,
};
use ryframe_application::ports::backup::BackupHealthCache;
use ryframe_config::{AppConfig, Environment};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::{NEW_PASSWORD, configuration_fixture, initialize, message, mysql, sql_initialization};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tenant_without_super_role_assigns_roles_through_http_and_preserves_boundaries() {
    if !mysql::integration_enabled() {
        eprintln!(
            "跳过角色分配 HTTP/MySQL 回归（未验收）：必须存在显式 .local-tests/mysql/config 配置目录"
        );
        return;
    }
    initialize();
    mysql::run_mysql_test("tenant-role-http", |database| async move {
        let exported = sql_initialization::import(&database).await?;
        seed_tenant(&database).await?;
        let (state, _lifetime) = state(&database).await?;
        let admin = state.services.identity.auth.login("role-tenant", "tenant_admin", NEW_PASSWORD)
            .await.map_err(message)?;
        assert!(!admin.user_info.is_super_admin);
        assert!(admin.user_info.perms.iter().any(|permission| permission == "sys:role:editSuper"));
        let ordinary = state.services.identity.auth.login("role-tenant", "ordinary", NEW_PASSWORD)
            .await.map_err(message)?;
        let server = serve(state.clone()).await?;
        assert_eq!(request(server.address, &admin.access_token, 101, &[111, 112]).await?, 200);
        let assigned = assignments(&database, 101).await?;
        assert_eq!(assigned.0, vec![111, 112]);
        assert_eq!(assigned.1, 2);
        assert_eq!(request(server.address, &ordinary.access_token, 101, &[111]).await?, 401);
        assert_eq!(assignments(&database, 101).await?, assigned);
        rejected_assignments(&database, &server, &admin.access_token).await?;

        // 权威 super 身份继续具有全权限，不依赖额外的 editSuper 字符串权限。
        mysql::execute(&database, "UPDATE sys_user SET password_change_required=0 WHERE id=1").await?;
        let system = state.services.identity.auth.login("system", "admin", sql_initialization::ADMIN_PASSWORD)
            .await.map_err(message)?;
        assert!(system.user_info.is_super_admin);
        assert_eq!(request(server.address, &system.access_token, 2, &[1, 2]).await?, 200);
        let system_roles = database.query_all_raw(Statement::from_string(DbBackend::MySql,
            "SELECT role_id FROM sys_user_role WHERE tenant_id='system' AND user_id=2 ORDER BY role_id"))
            .await.map_err(message)?;
        assert_eq!(system_roles.into_iter().map(|row| row.try_get::<i64>("", "role_id").unwrap())
            .collect::<Vec<_>>(), vec![1, 2]);
        drop(server);
        exported.finish()
    }).await;
}

async fn rejected_assignments(
    database: &DatabaseConnection,
    server: &Server,
    token: &str,
) -> Result<(), String> {
    let ordinary = assignments(database, 101).await?;
    let admin = assignments(database, 100).await?;
    // 自改角色仍须独立授权，不能因删除 super 查询绕过。
    assert_eq!(request(server.address, token, 100, &[110, 111]).await?, 403);
    assert_eq!(assignments(database, 100).await?, admin);
    for roles in [&[111, 2][..], &[111, 999_999][..]] {
        assert_eq!(request(server.address, token, 101, roles).await?, 400);
        assert_eq!(assignments(database, 101).await?, ordinary);
    }
    mysql::execute(database, "INSERT INTO sys_role (id,tenant_id,name,code,is_super) VALUES (120,'role-tenant','受保护角色','protected',1)").await?;
    // 即使普通管理员具有 editSuper 权限，应用层仍拒绝授予同租户 super。
    assert_eq!(request(server.address, token, 101, &[111, 120]).await?, 403);
    assert_eq!(assignments(database, 101).await?, ordinary);
    Ok(())
}

async fn seed_tenant(database: &DatabaseConnection) -> Result<(), String> {
    for sql in [
        "INSERT INTO sys_tenant (id,tenant_id,name) VALUES (100,'role-tenant','无超级角色租户')",
        "INSERT INTO sys_role (id,tenant_id,name,code) VALUES (110,'role-tenant','租户管理员','tenant_admin'),(111,'role-tenant','普通角色','ordinary'),(112,'role-tenant','业务角色','business')",
        "INSERT INTO sys_permission (id,tenant_id,name,code,perm_type) VALUES (900010,'role-tenant','修改用户','system:user:edit','api'),(900011,'role-tenant','修改超级角色','sys:role:editSuper','api')",
        "INSERT INTO sys_role_permission (tenant_id,role_id,perm_id) VALUES ('role-tenant',110,900010),('role-tenant',110,900011),('role-tenant',111,900010)",
    ] {
        mysql::execute(database, sql).await?;
    }
    let hash = ryframe_auth::password::hash(NEW_PASSWORD).map_err(message)?;
    database.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "INSERT INTO sys_user (id,tenant_id,username,password_hash,nickname) VALUES (100,'role-tenant','tenant_admin',?,'租户管理员'),(101,'role-tenant','ordinary',?,'普通用户')",
        [hash.clone().into(), hash.into()])).await.map_err(message)?;
    mysql::execute(database, "INSERT INTO sys_user_role (tenant_id,user_id,role_id) VALUES ('role-tenant',100,110),('role-tenant',101,111)").await
}

async fn assignments(database: &DatabaseConnection, user: i64) -> Result<(Vec<i64>, i32), String> {
    let roles = database.query_all_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT role_id FROM sys_user_role WHERE tenant_id='role-tenant' AND user_id=? ORDER BY role_id",
        [user.into()])).await.map_err(message)?;
    let roles = roles
        .into_iter()
        .map(|row| row.try_get("", "role_id").map_err(message))
        .collect::<Result<Vec<i64>, _>>()?;
    let user = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT authorization_version FROM sys_user WHERE tenant_id='role-tenant' AND id=?",
            [user.into()],
        ))
        .await
        .map_err(message)?
        .ok_or("缺少测试用户")?;
    Ok((
        roles,
        user.try_get("", "authorization_version").map_err(message)?,
    ))
}

struct Lifetime {
    shutdown: tokio::sync::watch::Sender<bool>,
    _directory: tempfile::TempDir,
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn state(database: &DatabaseConnection) -> Result<(AppState, Lifetime), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
    std::fs::create_dir_all(&root).map_err(message)?;
    let directory = tempfile::Builder::new()
        .prefix("role-http-")
        .tempdir_in(root)
        .map_err(message)?;
    configuration_fixture::write_configuration(directory.path(), 18080, 18081, None);
    let mut config = AppConfig::load(directory.path(), Environment::Test).map_err(message)?;
    config.multi_tenancy.enabled = true;
    config.object_storage.local_base_dir = directory
        .path()
        .join("storage")
        .to_string_lossy()
        .into_owned();
    let database = ryframe_db::ControlDatabaseCluster::single(database.clone());
    let router =
        Arc::new(boot::tenant_data::build_router(database.clone(), &config).map_err(message)?);
    let limiter = boot::limiter::init(&config, &None, false)
        .map_err(message)?
        .limiter;
    let policies =
        boot::application_policy::ApplicationPolicies::from_config(&config).map_err(message)?;
    let services = boot::services::build_all(
        &database,
        router,
        &config,
        &policies,
        boot::services::ServiceInfrastructure {
            redis_client: &None,
            object_storage: boot::storage::init(&config, true).await.map_err(message)?,
            rate_limiter: limiter.clone(),
            starts_background_tasks: false,
        },
    )
    .await
    .map_err(message)?;
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let (server_info, _sampler) = ServerInfoSampler::spawn(receiver).await.map_err(message)?;
    let state = boot::app_state::assemble(AppStateAssembly {
        database,
        config: Arc::new(config),
        localizer: Arc::new(LocalizerLoader::embedded().map_err(message)?),
        redis_client: None,
        token_blacklist: TokenBlacklist::new(None),
        services,
        limiter,
        server_info,
        backup_health: BackupHealthCache::new(boot::backup::CACHE_MAX_AGE),
    });
    Ok((
        state,
        Lifetime {
            shutdown,
            _directory: directory,
        },
    ))
}

struct Server {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(state: AppState) -> Result<Server, String> {
    let app = Router::new()
        .nest("/api/v1/system/users", user_router(state.clone()))
        .layer(from_fn_with_state(state.auth, auth_middleware));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(message)?;
    let address = listener.local_addr().map_err(message)?;
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Ok(Server { address, task })
}

async fn request(
    address: SocketAddr,
    token: &str,
    user: i64,
    roles: &[i64],
) -> Result<u16, String> {
    let body = serde_json::json!({ "role_ids": roles.iter().map(ToString::to_string).collect::<Vec<_>>() }).to_string();
    let request = format!(
        "PUT /api/v1/system/users/{user}/roles HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::task::spawn_blocking(move || {
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(message)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(message)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(message)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| "写入测试 HTTP 请求失败".to_owned())?;
        let mut response = String::new();
        stream
            .take(1024 * 1024)
            .read_to_string(&mut response)
            .map_err(message)?;
        response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or("缺少 HTTP 状态码")?
            .parse::<u16>()
            .map_err(message)
    })
    .await
    .map_err(message)?
}

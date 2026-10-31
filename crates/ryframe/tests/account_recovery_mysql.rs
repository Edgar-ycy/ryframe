use std::sync::{
    Arc, Once,
    atomic::{AtomicI64, Ordering},
};

use futures_util::FutureExt;
use redis::AsyncCommands;
use ryframe_adapters::RedisClient;
use ryframe_application::{
    AuthPolicy, AuthService, AuthorizationCache, CacheAvailabilityPolicy, PrincipalResolver,
    system::identity::{
        CreateUserParams, ProfileService, USER_STATUS_DISABLED, USER_STATUS_MUST_RESET_PASSWORD,
        USER_STATUS_NORMAL, USER_STATUS_PENDING_ACTIVATION, UserService,
    },
};
use ryframe_auth::{jwt::TokenSettings, password};
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use ryframe_kernel::{ActorContext, DataScope};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

#[path = "account_recovery_mysql/imported_accounts/http.rs"]
mod account_http;
#[path = "account_recovery_mysql/cache_faults.rs"]
mod cache_faults;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;
#[path = "account_recovery_mysql/imported_accounts.rs"]
mod imported_accounts;
#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;
#[path = "../../ryframe-adapters/tests/support/configuration.rs"]
mod redis_configuration;
#[path = "account_recovery_mysql/role_assignment.rs"]
mod role_assignment;
#[path = "account_recovery_mysql/scope_coherence.rs"]
mod scope_coherence;
#[path = "account_recovery_mysql/sql_initialization.rs"]
mod sql_initialization;

const OLD_PASSWORD: &str = "AbandonedRandom123!";
const NEW_PASSWORD: &str = "RecoveredUser123!";
static NEXT_ID: AtomicI64 = AtomicI64::new(1_000_000);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_user_real_redis_repairs_lagging_authorization_after_password_change() {
    let mysql_enabled = mysql::integration_enabled();
    let redis_enabled = redis_configuration::enabled("redis");
    if !mysql_enabled || !redis_enabled {
        eprintln!(
            "跳过 MySQL+Redis 首次改密和授权缓存真实协议测试（未验收）：必须同时存在显式 .local-tests/mysql/config 和 .local-tests/service-tests/redis/config 配置目录"
        );
        return;
    }
    initialize();
    let configuration = redis_configuration::Configuration::load("redis", "account-recovery");
    let config = configuration
        .app
        .redis
        .as_ref()
        .expect("Redis 测试配置缺少 redis 字段；请修复显式文件配置");
    let client = RedisClient::connect(config)
        .await
        .unwrap_or_else(|_| panic!("Redis 连接失败；请核对显式文件配置、TLS 和服务状态"));
    let redis = redis_configuration::ScopedClient {
        client,
        configuration,
    };
    mysql::run_mysql_test("account-redis", |database| async {
        let mut subjects = vec![2];
        // 无论断言失败或返回错误，都先精确清理已登记主体的授权键，再交回 MySQL fixture 保留证据。
        let result = std::panic::AssertUnwindSafe(real_redis_password_change(
            database,
            &redis,
            &mut subjects,
        ))
        .catch_unwind()
        .await;
        cleanup_authorization_keys(&redis, &subjects).await?;
        match result {
            Ok(Ok(())) => {
                let remaining = redis
                    .scan_keys("*")
                    .await
                    .map_err(|_| "复核本次授权缓存测试 scope 清理失败；保留现场，禁止自动重放")?;
                if !remaining.is_empty() {
                    return Err(format!(
                        "授权缓存测试清理失败：本次 scope 仍残留 {} 个键；保留现场，禁止自动重放",
                        remaining.len()
                    ));
                }
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
    .await;
}

async fn real_redis_password_change(
    database: DatabaseConnection,
    redis: &redis_configuration::ScopedClient<RedisClient>,
    subjects: &mut Vec<i64>,
) -> Result<(), String> {
    let hashes = migration::BootstrapHashes::new(
        password::hash("ExplicitAdmin123!").map_err(message)?,
        password::hash("123456").map_err(message)?,
    )
    .map_err(message)?;
    migration::up(&database, Some(&hashes))
        .await
        .map_err(message)?;
    let cache = ryframe::boot::authorization_cache::cache(
        Some(redis.client.clone()),
        CacheAvailabilityPolicy::Required,
    );
    let (_, auth) = services_with_cache(database.clone(), cache.clone())?;
    let auth = Arc::new(auth);
    let initial = auth
        .login("system", "user", "123456")
        .await
        .map_err(message)?;
    assert!(initial.user_info.password_change_required);
    let claims = ryframe_auth::jwt::decode_token(&initial.access_token, &auth.token_settings())
        .map_err(message)?;
    let initial_version = claims.user_authorization_version;
    let principal = auth.resolve_principal(&claims).await.map_err(message)?;
    assert!(principal.password_change_required);
    let profile = ProfileService::new(
        application_ports::users::profile(
            ControlDatabaseCluster::single(database.clone()),
            cache.clone(),
        ),
        cache.clone(),
    );
    profile
        .change_password(&principal.actor, "123456", "FirstLogin123!")
        .await
        .map_err(message)?;
    assert_session_rejected(&auth, &initial).await;
    assert!(auth.login("system", "user", "123456").await.is_err());
    let changed = auth
        .login("system", "user", "FirstLogin123!")
        .await
        .map_err(message)?;
    assert!(!changed.user_info.password_change_required);
    let claims = ryframe_auth::jwt::decode_token(&changed.access_token, &auth.token_settings())
        .map_err(message)?;
    mysql::require_count(
        &database,
        "SELECT authorization_version AS value FROM sys_user WHERE id=2",
        i64::from(claims.user_authorization_version),
    )
    .await?;
    assert!(claims.user_authorization_version > initial_version);
    assert_lagging_cache_repaired(redis, &cache, &auth, &claims).await?;
    assert_session_rejected(&auth, &initial).await;
    cache_faults::verify(&database, redis, &cache, auth.clone(), &changed).await?;
    scope_coherence::verify(&database, &cache, &auth, &claims, subjects).await?;
    Ok(())
}

async fn assert_lagging_cache_repaired(
    redis: &RedisClient,
    cache: &AuthorizationCache,
    auth: &AuthService,
    claims: &ryframe_auth::jwt::Claims,
) -> Result<(), String> {
    auth.resolve_principal(claims).await.map_err(message)?;
    assert!(
        cache
            .lookup_snapshot("system", 2)
            .await
            .map_err(message)?
            .snapshot
            .is_some()
    );
    let mut connection = redis.conn().clone();
    let key = redis
        .scoped_key(&ryframe::boot::authorization_cache_keyspace::user_version_key("system", 2));
    let lagging = claims.user_authorization_version - 1;
    let _: () = connection
        .set(&key, lagging)
        .await
        .map_err(|_| "写入授权版本落后夹具失败")?;
    assert_eq!(
        cache
            .lookup_snapshot("system", 2)
            .await
            .map_err(message)?
            .user_authorization_version,
        Some(lagging)
    );
    let principal = auth.resolve_principal(claims).await.map_err(message)?;
    assert_eq!(principal.actor.user_id, 2);
    assert!(!principal.password_change_required);
    assert_eq!(
        principal.user_authorization_version,
        claims.user_authorization_version
    );
    let repaired = cache.lookup_snapshot("system", 2).await.map_err(message)?;
    assert_eq!(
        repaired.user_authorization_version,
        Some(claims.user_authorization_version)
    );
    let snapshot = repaired
        .snapshot
        .expect("数据库权威版本必须重新写入真实 Redis 快照");
    assert_eq!(
        snapshot.versions.user_authorization_version,
        claims.user_authorization_version
    );
    let persisted: i32 = connection
        .get(&key)
        .await
        .map_err(|_| "读取修复后的授权版本失败")?;
    assert_eq!(persisted, claims.user_authorization_version);
    auth.resolve_principal(claims).await.map_err(message)?;
    Ok(())
}

async fn cleanup_authorization_keys(redis: &RedisClient, subjects: &[i64]) -> Result<(), String> {
    use ryframe::boot::authorization_cache_keyspace::{
        snapshot_hash_key, tenant_epoch_key, user_version_key,
    };
    let mut keys = vec![redis.scoped_key(&tenant_epoch_key("system"))];
    for &subject in subjects {
        keys.extend(
            [
                user_version_key("system", subject),
                snapshot_hash_key("system", subject),
            ]
            .map(|key| redis.scoped_key(&key)),
        );
    }
    let mut connection = redis.conn().clone();
    let _: usize = connection
        .del(&keys)
        .await
        .map_err(|_| "精确清理本次 scope 的授权键失败；保留现场，不自动重放")?;
    let remaining: usize = connection
        .exists(&keys)
        .await
        .map_err(|_| "复核授权键清理失败；保留现场，不自动重放")?;
    assert_eq!(remaining, 0, "本次 scope 已登记主体的授权键必须全部清理");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_user_login_requires_and_completes_password_change() {
    if !mysql::integration_enabled() {
        eprintln!(
            "跳过初始用户首次改密真实协议测试：缺少显式 .local-tests/mysql/config 配置目录（未验收）"
        );
        return;
    }
    initialize();
    mysql::run_mysql_test("initial-user-login", |database| async move {
        let exported = sql_initialization::import(&database).await?;
        let (_, auth) = services(database.clone())?;
        let initial = auth
            .login("system", "user", "123456")
            .await
            .map_err(message)?;
        assert!(initial.user_info.password_change_required);

        let cache = AuthorizationCache::disabled();
        let profile = ProfileService::new(
            application_ports::users::profile(
                ControlDatabaseCluster::single(database.clone()),
                cache.clone(),
            ),
            cache,
        );
        let user = ActorContext {
            user_id: 2,
            username: "user".into(),
            is_super_admin: false,
            data_scope: DataScope::SelfOnly,
            ..administrator()
        };
        profile
            .change_password(&user, "123456", "FirstLogin123!")
            .await
            .map_err(message)?;
        assert_session_rejected(&auth, &initial).await;
        assert!(auth.login("system", "user", "123456").await.is_err());
        let changed = auth
            .login("system", "user", "FirstLogin123!")
            .await
            .map_err(message)?;
        assert!(!changed.user_info.password_change_required);
        let refreshed = auth
            .refresh_token(&changed.refresh_token, "first-login-changed")
            .await
            .map_err(message)?;
        assert!(!refreshed.user_info.password_change_required);

        migration::up(&database, None).await.map_err(message)?;
        assert!(
            !auth
                .login("system", "user", "FirstLogin123!")
                .await
                .map_err(message)?
                .user_info
                .password_change_required
        );
        let admin = auth
            .login("system", "admin", sql_initialization::ADMIN_PASSWORD)
            .await
            .map_err(message)?;
        assert!(admin.user_info.password_change_required);
        assert!(auth.login("system", "admin", "123456").await.is_err());
        profile
            .change_password(
                &administrator(),
                sql_initialization::ADMIN_PASSWORD,
                "ChangedAdmin123!",
            )
            .await
            .map_err(message)?;
        assert_session_rejected(&auth, &admin).await;
        assert!(
            auth.login("system", "admin", sql_initialization::ADMIN_PASSWORD)
                .await
                .is_err()
        );
        assert!(
            !auth
                .login("system", "admin", "ChangedAdmin123!")
                .await
                .map_err(message)?
                .user_info
                .password_change_required
        );
        exported.finish()?;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_abnormal_accounts_recover_explicitly_and_old_credentials_stay_invalid() {
    if !mysql::integration_enabled() {
        eprintln!(
            "跳过异常账号恢复真实协议测试：缺少显式 .local-tests/mysql/config 配置目录（未验收）"
        );
        return;
    }
    initialize();
    for status in [
        USER_STATUS_NORMAL,
        USER_STATUS_PENDING_ACTIVATION,
        USER_STATUS_MUST_RESET_PASSWORD,
        USER_STATUS_DISABLED,
    ] {
        println!("→ 异常账号恢复：原状态={status}");
        mysql::run_mysql_test("account-recovery", |database| async move {
            let hashes = migration::BootstrapHashes::new(
                password::hash("ExplicitAdmin123!").map_err(message)?,
                password::hash("123456").map_err(message)?,
            )
            .map_err(message)?;
            migration::up(&database, Some(&hashes))
                .await
                .map_err(message)?;
            prepare(&database, status).await?;
            let (users, auth) = services(database.clone())?;
            recover(&database, &users, &auth, status).await?;
            // 重新启动/校验初始化不会重置已选密码和改密标志。
            migration::up(&database, None).await.map_err(message)?;
            let (_, restarted) = services(database.clone())?;
            let login = restarted
                .login("system", "user", NEW_PASSWORD)
                .await
                .map_err(message)?;
            assert!(!login.user_info.password_change_required);
            assert!(restarted.login("system", "user", "123456").await.is_err());
            Ok(())
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_reset_races_and_failures_keep_credentials_atomic() {
    if !mysql::integration_enabled() {
        eprintln!(
            "跳过密码重置并发与事务真实协议测试：缺少显式 .local-tests/mysql/config 配置目录（未验收）"
        );
        return;
    }
    initialize();
    mysql::run_mysql_test("account-reset-race", |database| async move {
        let hashes = migration::BootstrapHashes::new(
            password::hash("ExplicitAdmin123!").map_err(message)?,
            password::hash("123456").map_err(message)?,
        )
        .map_err(message)?;
        migration::up(&database, Some(&hashes))
            .await
            .map_err(message)?;
        prepare(&database, USER_STATUS_NORMAL).await?;
        let (users, auth) = services(database.clone())?;
        expiry_failure_rolls_back(&database, &users).await?;
        competing_resets(&users, &auth).await?;
        self_change_expires_requests(&database, &users, &auth).await
    })
    .await;
}

fn initialize() {
    static INITIALIZED: Once = Once::new();
    INITIALIZED.call_once(|| {
        ryframe::crypto::install_crypto_provider().unwrap();
        ryframe_application::install_id_generator(|| Ok(NEXT_ID.fetch_add(1, Ordering::Relaxed)))
            .unwrap();
        ryframe_db::install_id_generator(|| Ok(NEXT_ID.fetch_add(1, Ordering::Relaxed))).unwrap();
    });
}

async fn recover(
    database: &DatabaseConnection,
    users: &UserService,
    auth: &AuthService,
    status: &str,
) -> Result<(), String> {
    let actor = administrator();
    let other = another_account(users, &actor).await?;
    assert!(auth.login("system", "user", "123456").await.is_err());
    let old_session = if status == USER_STATUS_NORMAL {
        users
            .update_status(&actor, 2, USER_STATUS_DISABLED.into())
            .await
            .map_err(message)?;
        users
            .update_status(&actor, 2, USER_STATUS_NORMAL.into())
            .await
            .map_err(message)?;
        let login = auth
            .login("system", "user", OLD_PASSWORD)
            .await
            .map_err(message)?;
        assert!(
            login.user_info.password_change_required,
            "管理员启用不能清除改密要求"
        );
        Some(login)
    } else {
        None
    };
    let older = users
        .request_password_reset(&actor, 2, "恢复异常开发账号", None)
        .await
        .map_err(message)?;
    let selected = users
        .request_password_reset(&actor, 2, "用户明确选择本次恢复请求", None)
        .await
        .map_err(message)?;
    users
        .complete_password_reset_request(
            "system",
            selected.request.id,
            &selected.token,
            NEW_PASSWORD,
        )
        .await
        .map_err(message)?;
    assert!(
        users
            .complete_password_reset_request(
                "system",
                selected.request.id,
                &selected.token,
                "ReplayUser123!"
            )
            .await
            .is_err()
    );
    assert!(
        users
            .complete_password_reset_request(
                "system",
                older.request.id,
                &older.token,
                "ReplayUser123!"
            )
            .await
            .is_err(),
        "完成改密必须同时使此前签发的其他重置令牌失效"
    );
    users
        .complete_password_reset_request(
            "system",
            other.request.id,
            &other.token,
            "OtherChosen123!",
        )
        .await
        .map_err(message)?;
    assert!(
        !auth
            .login("system", "unrelated-user", "OtherChosen123!")
            .await
            .map_err(message)?
            .user_info
            .password_change_required
    );
    verify_recovered(database, users, auth, status, old_session).await
}

async fn verify_recovered(
    database: &DatabaseConnection,
    users: &UserService,
    auth: &AuthService,
    status: &str,
    old_session: Option<ryframe_application::LoginResult>,
) -> Result<(), String> {
    let actor = administrator();
    if let Some(session) = old_session {
        assert_session_rejected(auth, &session).await;
    }
    if status == USER_STATUS_DISABLED {
        assert!(
            auth.login("system", "user", NEW_PASSWORD).await.is_err(),
            "重置不能自动启用停用账号"
        );
        users
            .update_status(&actor, 2, USER_STATUS_NORMAL.into())
            .await
            .map_err(message)?;
    }
    let login = auth
        .login("system", "user", NEW_PASSWORD)
        .await
        .map_err(message)?;
    assert!(!login.user_info.password_change_required);
    assert_eq!(login.user_info.tenant_id, "system");
    let refreshed = auth
        .refresh_token(&login.refresh_token, "recovery-valid-session")
        .await
        .map_err(message)?;
    assert!(!refreshed.user_info.password_change_required);
    assert!(auth.login("system", "user", OLD_PASSWORD).await.is_err());
    mysql::require_count(database, "SELECT COUNT(*) AS value FROM sys_user WHERE id=2 AND status='1' AND password_change_required=0", 1).await
}

async fn assert_session_rejected(auth: &AuthService, session: &ryframe_application::LoginResult) {
    let claims =
        ryframe_auth::jwt::decode_token(&session.access_token, &auth.token_settings()).unwrap();
    assert!(auth.resolve_principal(&claims).await.is_err());
    assert!(
        auth.refresh_token(&session.refresh_token, "previous-password-session")
            .await
            .is_err()
    );
}

async fn expiry_failure_rolls_back(
    database: &DatabaseConnection,
    users: &UserService,
) -> Result<(), String> {
    let actor = administrator();
    let _older = users
        .request_password_reset(&actor, 2, "保留尚有效的旧请求", None)
        .await
        .map_err(message)?;
    let selected = users
        .request_password_reset(&actor, 2, "事务故障核验", None)
        .await
        .map_err(message)?;
    let before = credential_state(database).await?;
    database.execute_unprepared("CREATE TRIGGER fixture_reset_expiry BEFORE UPDATE ON password_reset_requests FOR EACH ROW BEGIN IF NEW.status='expired' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='fixture_expiry_failure'; END IF; END").await.map_err(message)?;
    assert!(
        users
            .complete_password_reset_request(
                "system",
                selected.request.id,
                &selected.token,
                NEW_PASSWORD
            )
            .await
            .is_err()
    );
    assert!(
        before == credential_state(database).await?,
        "重置失败必须回滚密码、版本及所有请求状态"
    );
    let cache = AuthorizationCache::disabled();
    let profile = ProfileService::new(
        application_ports::users::profile(
            ControlDatabaseCluster::single(database.clone()),
            cache.clone(),
        ),
        cache,
    );
    let user = ActorContext {
        user_id: 2,
        username: "user".into(),
        is_super_admin: false,
        data_scope: DataScope::SelfOnly,
        ..actor
    };
    assert!(
        profile
            .change_password(&user, OLD_PASSWORD, NEW_PASSWORD)
            .await
            .is_err()
    );
    assert!(
        before == credential_state(database).await?,
        "自助改密失败必须回滚完整认证前后像"
    );
    database
        .execute_unprepared("DROP TRIGGER fixture_reset_expiry")
        .await
        .map(|_| ())
        .map_err(message)
}

async fn credential_state(database: &DatabaseConnection) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CAST(JSON_OBJECT('hash',password_hash,'version',authorization_version,'required',password_change_required,'status',status,'updated',updated_at) AS CHAR) AS value FROM sys_user WHERE id=2",
        "SELECT CAST(JSON_OBJECT('id',id,'hash',token_hash,'status',status,'expires',expires_at,'completed',completed_at,'updated',updated_at) AS CHAR) AS value FROM password_reset_requests WHERE target_user_id=2 ORDER BY id",
    ] {
        for row in database
            .query_all_raw(Statement::from_string(DbBackend::MySql, sql))
            .await
            .map_err(message)?
        {
            result.push(row.try_get("", "value").map_err(message)?);
        }
    }
    Ok(result)
}

async fn another_account(
    users: &UserService,
    actor: &ActorContext,
) -> Result<ryframe_application::system::identity::PasswordResetRequestOutcome, String> {
    let other = users
        .create(
            actor,
            CreateUserParams {
                username: "unrelated-user",
                nickname: "其他普通用户",
                email: "other@example.invalid",
                phone: "13800000002",
                dept_id: Some(5),
                role_ids: vec![2],
            },
        )
        .await
        .map_err(message)?;
    let id = other.id.parse::<i64>().map_err(message)?;
    users
        .request_password_reset(actor, id, "独立账号重置不受其他用户影响", None)
        .await
        .map_err(message)
}

async fn competing_resets(users: &UserService, auth: &AuthService) -> Result<(), String> {
    let actor = administrator();
    let first = users
        .request_password_reset(&actor, 2, "并发请求一", None)
        .await
        .map_err(message)?;
    let second = users
        .request_password_reset(&actor, 2, "并发请求二", None)
        .await
        .map_err(message)?;
    let (one, two) = tokio::join!(
        users.complete_password_reset_request(
            "system",
            first.request.id,
            &first.token,
            NEW_PASSWORD
        ),
        users.complete_password_reset_request(
            "system",
            second.request.id,
            &second.token,
            "RaceUser123!"
        ),
    );
    assert_ne!(
        one.is_ok(),
        two.is_ok(),
        "同一代凭据签发的两个重置请求只能有一个完成"
    );
    let winner = if one.is_ok() {
        NEW_PASSWORD
    } else {
        "RaceUser123!"
    };
    let loser = if one.is_ok() {
        "RaceUser123!"
    } else {
        NEW_PASSWORD
    };
    assert!(auth.login("system", "user", winner).await.is_ok());
    assert!(auth.login("system", "user", loser).await.is_err());
    // 新签发的请求仍可使用，不因上一代失效而永久阻断恢复。
    let fresh = users
        .request_password_reset(&actor, 2, "后续明确恢复", None)
        .await
        .map_err(message)?;
    users
        .complete_password_reset_request("system", fresh.request.id, &fresh.token, NEW_PASSWORD)
        .await
        .map_err(message)?;
    Ok(())
}

async fn self_change_expires_requests(
    database: &DatabaseConnection,
    users: &UserService,
    auth: &AuthService,
) -> Result<(), String> {
    let admin = administrator();
    let stale = users
        .request_password_reset(&admin, 2, "用户自助改密前签发", None)
        .await
        .map_err(message)?;
    let old_session = auth
        .login("system", "user", NEW_PASSWORD)
        .await
        .map_err(message)?;
    let actor = ActorContext {
        user_id: 2,
        username: "user".into(),
        is_super_admin: false,
        data_scope: DataScope::SelfOnly,
        ..admin
    };
    let cache = AuthorizationCache::disabled();
    let profile = ProfileService::new(
        application_ports::users::profile(
            ControlDatabaseCluster::single(database.clone()),
            cache.clone(),
        ),
        cache,
    );
    profile
        .change_password(&actor, NEW_PASSWORD, "SelfChosen123!")
        .await
        .map_err(message)?;
    assert!(
        users
            .complete_password_reset_request(
                "system",
                stale.request.id,
                &stale.token,
                "ReplayUser123!"
            )
            .await
            .is_err()
    );
    assert_session_rejected(auth, &old_session).await;
    assert!(
        !auth
            .login("system", "user", "SelfChosen123!")
            .await
            .map_err(message)?
            .user_info
            .password_change_required
    );
    Ok(())
}

async fn prepare(database: &DatabaseConnection, status: &str) -> Result<(), String> {
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE sys_user SET password_hash=?, status=?, password_change_required=1 WHERE id=2",
            [
                password::hash(OLD_PASSWORD).map_err(message)?.into(),
                status.into(),
            ],
        ))
        .await
        .map_err(message)?;
    // 隔离夹具中管理员已完成自己的初始改密；恢复只作用于显式选择的普通账号。
    mysql::execute(
        database,
        "UPDATE sys_user SET password_change_required=0 WHERE id=1",
    )
    .await
}

fn services(database: DatabaseConnection) -> Result<(UserService, AuthService), String> {
    services_with_cache(database, AuthorizationCache::disabled())
}

fn services_with_cache(
    database: DatabaseConnection,
    cache: AuthorizationCache,
) -> Result<(UserService, AuthService), String> {
    let cluster = ControlDatabaseCluster::single(database);
    let identity = application_ports::auth::identity(cluster.clone());
    let users = UserService::new(
        cache.clone(),
        Arc::clone(&identity),
        application_ports::users::query(cluster.clone()),
        application_ports::users::write(cluster.clone(), cache.clone()),
        application_ports::auth::password_reset(cluster, cache.clone()),
    );
    let auth = AuthService::new(
        identity,
        AuthPolicy::new(5, 15).map_err(message)?,
        Arc::new(
            TokenSettings::new(
                "account-recovery-test-only-signing-key-at-least-32",
                "30m",
                "168h",
            )
            .map_err(message)?,
        ),
        ryframe::boot::login_protection::store(None),
        ryframe::boot::refresh_sessions::store(None),
        cache,
    );
    Ok((users, auth))
}

fn administrator() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "system".into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: vec![],
        include_self: true,
        data_scope_version: 1,
        is_super_admin: true,
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

//! MySQL 真实协议测试共用的精确隔离 fixture；只有显式文件配置才连接外部服务。
use futures_util::FutureExt;
use ryframe_config::{
    AppConfig, DatabaseConfig, DbConnection, DbTlsMode, Environment, SqlLogLevel,
};
use ryframe_db::connection;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, sqlx};
use std::{
    fs,
    future::Future,
    io,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static SCHEMA_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[path = "mysql_fixture_ownership.rs"]
mod ownership;
use ownership::{
    AdminSession, Evidence, Ownership, Plan, business_identity_matches, validate_schema,
};

pub(crate) struct MySqlTestConfiguration {
    config: AppConfig,
}

impl MySqlTestConfiguration {
    pub(crate) fn load() -> Result<Self, String> {
        let config = load_configuration_at(&configuration_directory()?)?;
        Ok(Self { config })
    }

    pub(crate) fn scope_id(&self) -> &str {
        self.config.scope_id.as_str()
    }

    pub(crate) fn database(&self) -> &DatabaseConfig {
        &self.config.database
    }

    pub(crate) fn connection(
        &self,
        database: &str,
        tls_mode: Option<DbTlsMode>,
    ) -> Result<DbConnection, String> {
        connection_config(&self.database().primary, database, tls_mode)
    }
}

struct MySqlFixture {
    admin: AdminSession,
    database: DatabaseConnection,
    ownership: Ownership,
    evidence: Evidence,
}

impl MySqlFixture {
    async fn create(
        test_name: &str,
        scope: &str,
        connection: &DbConnection,
        expected_server_uuid: Option<&str>,
    ) -> Result<Self, String> {
        let root = workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")))?;
        let schema = unique_schema(scope, test_name);
        validate_schema(&schema)?;
        let admin_config = connection_config(connection, "mysql", None)?;
        let schema_config = connection_config(connection, &schema, None)?;
        let pool = connection::connect(&admin_config)
            .await
            .map_err(|_| "连接 MySQL 管理库失败，未创建 schema".to_owned())?;
        let mut admin = AdminSession::acquire(pool).await?;
        let prepared = Self::prepare(
            &mut admin,
            scope,
            &schema,
            test_name,
            &schema_config,
            &root,
            expected_server_uuid,
        )
        .await;
        match prepared {
            Ok((database, ownership, evidence)) => Ok(Self {
                admin,
                database,
                ownership,
                evidence,
            }),
            Err(error) => {
                if let Err(release) = admin.release().await {
                    eprintln!("{release}");
                }
                Err(format!(
                    "{error}；schema {schema} 现场及 {} 中本次已创建的证据保留，未执行 DROP，不得自动重放",
                    root.join(".local-tests/mysql/fixtures").display()
                ))
            }
        }
    }

    async fn prepare(
        admin: &mut AdminSession,
        scope: &str,
        schema: &str,
        test_name: &str,
        schema_config: &DbConnection,
        root: &Path,
        expected_server_uuid: Option<&str>,
    ) -> Result<(DatabaseConnection, Ownership, Evidence), String> {
        let identity = admin.identity().await?;
        if expected_server_uuid
            .is_some_and(|expected| !identity.server_uuid.eq_ignore_ascii_case(expected))
        {
            return Err("fixture 管理连接实例与只读预检不符，尚未获取锁或建库".into());
        }
        admin.lock(schema).await?;
        let plan = Plan {
            schema: schema.to_owned(),
            scope: scope.to_owned(),
            test: test_name.to_owned(),
            identity,
        };
        let mut evidence = Evidence::create(root, &plan)?;
        let mut ownership = Ownership::new(plan)?;
        let result =
            Self::create_database(admin, &mut ownership, &mut evidence, schema_config).await;
        match result {
            Ok(database) => Ok((database, ownership, evidence)),
            Err(error) => {
                ownership.retain();
                evidence.failure("failure-create-or-connect");
                Err(error)
            }
        }
    }

    async fn create_database(
        admin: &mut AdminSession,
        ownership: &mut Ownership,
        evidence: &mut Evidence,
        schema_config: &DbConnection,
    ) -> Result<DatabaseConnection, String> {
        if admin.exists(&ownership.plan.schema).await? {
            return Err("拒绝已存在 schema；库名前缀不能证明归属".into());
        }
        ownership.create_intent()?;
        evidence.record("create-intent")?;
        admin.ddl(&ownership.plan.schema, true).await?;
        ownership.created()?;
        evidence.record("created")?;
        Self::connect_business(schema_config, &ownership.plan).await
    }

    async fn connect_business(
        schema_config: &DbConnection,
        plan: &Plan,
    ) -> Result<DatabaseConnection, String> {
        let mut options = connection::options(schema_config, SqlLogLevel::Off, 200);
        let server_uuid = plan.identity.server_uuid.clone();
        let schema = plan.schema.clone();
        options.map_sqlx_mysql_pool_opts(move |pool_options| {
            let server_uuid = server_uuid.clone();
            let schema = schema.clone();
            // 所有新建物理连接均须核对已锁定管理实例及本次 schema。
            pool_options.after_connect(move |connection, _metadata| {
                let server_uuid = server_uuid.clone();
                let schema = schema.clone();
                Box::pin(async move {
                    let (observed_uuid, observed_schema) = sqlx::query_as::<
                        _,
                        (String, Option<String>),
                    >(
                        sqlx::AssertSqlSafe("SELECT @@server_uuid, DATABASE()"),
                    )
                    .fetch_one(connection)
                    .await
                    .map_err(|_| sqlx::Error::Protocol("fixture 业务连接物理归属未确认".into()))?;
                    if !business_identity_matches(
                        &server_uuid,
                        &schema,
                        &observed_uuid,
                        observed_schema.as_deref(),
                    ) {
                        return Err(sqlx::Error::Protocol(
                            "fixture 业务连接物理归属未确认".into(),
                        ));
                    }
                    Ok(())
                })
            })
        });
        sea_orm::Database::connect(options)
            .await
            .map_err(|_| "连接新 schema 失败，保留已创建数据库".into())
    }

    async fn cleanup(self) -> Result<(), String> {
        let Self {
            mut admin,
            database,
            mut ownership,
            mut evidence,
        } = self;
        let result = async {
            database
                .close()
                .await
                .map_err(|_| "关闭业务连接池失败，禁止 DROP")?;
            ownership.test_passed()?;
            evidence.record("test-passed")?;
            Self::drop_database(&mut admin, &mut ownership, &mut evidence).await
        }
        .await;
        if result.is_err() {
            ownership.retain();
            evidence.failure("failure-cleanup-retained");
            eprintln!(
                "schema {} 的清理未完成；保留计划 {} 及同目录 journal，不宣称回滚，不重试 DROP",
                ownership.plan.schema,
                evidence.plan_path().display()
            );
        }
        let release = admin.release().await;
        if release.is_err() {
            evidence.failure("failure-admin-release");
        }
        match (result, release) {
            (Err(error), secondary) => {
                if let Err(error) = secondary {
                    eprintln!("{error}");
                }
                Err(error)
            }
            (Ok(()), result) => result,
        }
    }

    async fn drop_database(
        admin: &mut AdminSession,
        ownership: &mut Ownership,
        evidence: &mut Evidence,
    ) -> Result<(), String> {
        let observed = Self::await_quiescence(admin, ownership, evidence).await?;
        ownership.drop_intent(&observed)?;
        evidence.record("drop-intent")?;
        // 意图已落盘后再核对一次；任何失败均保留现场，不能重放该 DROP。
        evidence.verify()?;
        let observed = admin.observe(&ownership.plan.schema, true).await?;
        evidence.observed("cleanup-rechecked", &observed)?;
        ownership.validate_cleanup(&observed)?;
        evidence.verify()?;
        admin.ddl(&ownership.plan.schema, false).await?;
        let absent = !admin.exists(&ownership.plan.schema).await?;
        ownership.dropped(absent)?;
        evidence.record("dropped-confirmed")
    }

    async fn await_quiescence(
        admin: &mut AdminSession,
        ownership: &Ownership,
        evidence: &mut Evidence,
    ) -> Result<ownership::Observation, String> {
        // 连接池关闭后，服务端会话退役可能稍晚；只在首次 DROP 意图前观察。
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                evidence.verify()?;
                let observed = admin.observe(&ownership.plan.schema, true).await?;
                evidence.observed("cleanup-observed", &observed)?;
                if ownership.cleanup_quiescent(&observed)? {
                    return Ok(observed);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| "fixture 服务端连接退役观察超过 5 秒，保留 schema，禁止 DROP".to_string())?
    }

    async fn preserve(self, stage: &'static str) {
        let Self {
            admin,
            database,
            mut ownership,
            mut evidence,
        } = self;
        ownership.retain();
        evidence.failure(stage);
        eprintln!(
            "测试失败：schema {}、计划 {} 及同目录 journal 保留；未执行 DROP，不宣称回滚",
            ownership.plan.schema,
            evidence.plan_path().display()
        );
        if database.close().await.is_err() {
            evidence.failure("failure-business-close");
            eprintln!("业务连接关闭未确认，数据库现场仍保留");
        }
        if let Err(error) = admin.release().await {
            evidence.failure("failure-admin-release");
            eprintln!("{error}");
        }
    }
}

pub(crate) async fn run_mysql_test<F, Fut>(test_name: &str, test: F)
where
    F: FnOnce(DatabaseConnection) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if !integration_enabled() {
        eprintln!("跳过 MySQL 真实协议测试（未验收）：缺少显式 .local-tests/mysql/config 配置目录");
        return;
    }

    let config = MySqlTestConfiguration::load().unwrap_or_else(|error| panic!("{error}"));
    let primary = config
        .connection("mysql", None)
        .unwrap_or_else(|error| panic!("{error}"));
    run_mysql_test_with_configuration(test_name, config.scope_id(), &primary, None, test).await;
}

pub(crate) async fn run_mysql_test_with_configuration<F, Fut>(
    test_name: &str,
    scope: &str,
    connection: &DbConnection,
    expected_server_uuid: Option<&str>,
    test: F,
) where
    F: FnOnce(DatabaseConnection) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let fixture = MySqlFixture::create(test_name, scope, connection, expected_server_uuid)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let database = fixture.database.clone();
    let result = std::panic::AssertUnwindSafe(async move { test(database).await })
        .catch_unwind()
        .await;
    match result {
        Ok(Ok(())) => {
            if let Err(error) = fixture.cleanup().await {
                panic!("{error}");
            }
        }
        Ok(Err(error)) => {
            fixture.preserve("failure-test-error").await;
            panic!("{error}");
        }
        Err(payload) => {
            fixture.preserve("failure-test-panic").await;
            std::panic::resume_unwind(payload);
        }
    }
}

pub(crate) async fn execute<C>(connection: &C, sql: &str) -> Result<(), String>
where
    C: ConnectionTrait,
{
    connection
        .execute_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map(|_| ())
        .map_err(db_error)
}

pub(crate) async fn require_count<C>(connection: &C, sql: &str, expected: i64) -> Result<(), String>
where
    C: ConnectionTrait,
{
    let row = connection
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map_err(db_error)?
        .ok_or_else(|| format!("查询未返回结果: {sql}"))?;
    let actual = row
        .try_get::<i64>("", "value")
        .map_err(|error| format!("读取查询结果失败: {error}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "查询结果不匹配，期望 {expected}，实际 {actual}: {sql}"
        ))
    }
}

pub(crate) fn connection_config(
    primary: &DbConnection,
    database: &str,
    tls_mode: Option<DbTlsMode>,
) -> Result<DbConnection, String> {
    if primary.tls_mode == DbTlsMode::Disabled || tls_mode == Some(DbTlsMode::Disabled) {
        return Err("MySQL 真实协议测试必须启用 TLS；请修复 database.primary.tls_mode".into());
    }
    let mut config = primary.clone();
    config.database = database.to_owned();
    if let Some(tls_mode) = tls_mode {
        config.tls_mode = tls_mode;
    }
    Ok(config)
}

fn unique_schema(scope: &str, test_name: &str) -> String {
    let run = sanitized_fragment(scope, 16);
    let test = sanitized_fragment(test_name, 16);
    let sequence = SCHEMA_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("ryframe_it_{run}_{test}_{}_{sequence}", process::id())
}

fn sanitized_fragment(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter_map(|character| {
            let character = character.to_ascii_lowercase();
            character.is_ascii_alphanumeric().then_some(character)
        })
        .take(max_len)
        .collect::<String>()
}

pub(crate) fn integration_enabled() -> bool {
    configuration_directory()
        .and_then(|directory| integration_enabled_at(&directory))
        .unwrap_or_else(|error| panic!("{error}"))
}

fn configuration_directory() -> Result<PathBuf, String> {
    workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")))
        .map(|root| root.join(".local-tests/mysql/config"))
}

fn workspace_root(start: &Path) -> Result<PathBuf, String> {
    start
        .ancestors()
        .find(|path| path.join("architecture/crate-boundaries.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| "找不到后端 workspace 的 architecture/crate-boundaries.toml".into())
}

fn integration_enabled_at(directory: &Path) -> Result<bool, String> {
    // 从根逐层检查，不能把悬空链接、重解析点或无权限路径当作未配置。
    let mut missing = false;
    for path in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(path) {
            Ok(metadata) if ordinary(&metadata) && metadata.is_dir() => {}
            Ok(_) => return Err("MySQL 配置目录及其各层必须是普通无链接目录".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing = true,
            Err(_) => return Err("无法检查 MySQL 配置目录；请检查目录访问权限".into()),
        }
    }
    Ok(!missing)
}

fn load_configuration_at(directory: &Path) -> Result<AppConfig, String> {
    if !integration_enabled_at(directory)? {
        return Err("缺少 MySQL 显式配置目录；本次未验收".into());
    }
    for name in ["app.toml", "app.test.toml"] {
        let metadata = match fs::symlink_metadata(directory.join(name)) {
            Ok(metadata) => metadata,
            Err(error) if name == "app.toml" && error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "MySQL 配置缺少或无法读取 {name}（{}）；请提供普通无链接配置文件",
                    error.kind()
                ));
            }
        };
        if !ordinary(&metadata) || !metadata.is_file() {
            return Err(format!("MySQL 配置 {name} 必须是普通无链接文件"));
        }
    }
    let config = AppConfig::load(directory, Environment::Test)
        .map_err(|error| format!("MySQL 文件配置无效：{}", error.message()))?;
    if config.database.primary.tls_mode == DbTlsMode::Disabled {
        return Err("MySQL 真实协议测试必须启用 TLS；请修复 database.primary.tls_mode".into());
    }
    Ok(config)
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Windows 所有重解析点都拒绝，包括 junction。
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "mysql_configuration_tests.rs"]
mod configuration_tests;

pub(crate) fn db_error(error: DbErr) -> String {
    error.to_string()
}

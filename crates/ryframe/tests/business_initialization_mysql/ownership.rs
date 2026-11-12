use super::*;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn config(database: &str) -> Result<tempfile::TempDir, String> {
    let connection = mysql::MySqlTestConfiguration::load()?.connection(database, None)?;
    config_for(connection)
}

pub(super) fn config_for(
    connection: ryframe_config::DbConnection,
) -> Result<tempfile::TempDir, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".local-tests");
    fs::create_dir_all(&root).map_err(message)?;
    let directory = tempfile::Builder::new()
        .prefix("ownership-cli-")
        .tempdir_in(root)
        .map_err(message)?;
    configuration_fixture::write_configuration(
        directory.path(),
        8080,
        9091,
        Some("OwnershipInitial123!"),
    );
    let file = directory.path().join("app.toml");
    let source = fs::read_to_string(&file).map_err(message)?;
    let mut value: toml::Value = toml::from_str(&source).map_err(message)?;
    let primary = value["database"]["primary"]
        .as_table_mut()
        .ok_or("缺少primary")?;
    for (key, field) in [
        ("host", connection.host),
        ("database", connection.database),
        ("username", connection.username),
        ("password", connection.password),
    ] {
        primary.insert(key.into(), field.into());
    }
    primary.insert("port".into(), i64::from(connection.port).into());
    primary.insert("max_connections".into(), 1_i64.into());
    let tls = match connection.tls_mode {
        ryframe_config::DbTlsMode::Disabled => "disabled",
        ryframe_config::DbTlsMode::Required => "required",
        ryframe_config::DbTlsMode::VerifyCa => "verify_ca",
        ryframe_config::DbTlsMode::VerifyIdentity => "verify_identity",
    };
    primary.insert("tls_mode".into(), tls.into());
    for (key, field) in [
        ("tls_ca", connection.tls_ca),
        ("tls_client_cert", connection.tls_client_cert),
        ("tls_client_key", connection.tls_client_key),
    ] {
        if let Some(field) = field {
            primary.insert(key.into(), field.into());
        }
    }
    let mut reset = toml::map::Map::new();
    reset.insert("services_stopped".into(), true.into());
    value
        .as_table_mut()
        .ok_or("配置不是表")?
        .insert("reset".into(), reset.into());
    fs::write(file, toml::to_string(&value).map_err(message)?).map_err(message)?;
    Ok(directory)
}

fn command(directory: &Path, args: &[&str]) -> Command {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"));
    command
        .current_dir(root)
        .arg("--config-dir")
        .arg(directory)
        .args(args)
        .env("APP_ENV", "test")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

pub(super) fn run(directory: &Path, args: &[&str]) -> Output {
    bounded(ManagedChild(Some(
        command(directory, args).spawn().unwrap(),
    )))
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_control_and_shared_tenant_claim_separate_kinds_and_repeat_up() {
    mysql::run_mysql_test("ownerfresh", |db| async move {
        let identity = ryframe_db::connection::physical_identity(&db)
            .await
            .map_err(message)?;
        let config = config(&identity.database)?;
        success(run(config.path(), &["control", "up"]));
        mysql::require_count(
            &db,
            "SELECT COUNT(*) AS value FROM ryframe_resource_ownership",
            1,
        )
        .await?;
        success(run(
            config.path(),
            &["tenant-data", "up", "--target", "shared-control"],
        ));
        success(run(config.path(), &["control", "up"]));
        success(run(
            config.path(),
            &["tenant-data", "up", "--target", "shared-control"],
        ));
        mysql::require_count(
            &db,
            "SELECT COUNT(*) AS value FROM ryframe_resource_ownership",
            2,
        )
        .await?;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_unowned_database_requires_plan_and_rejects_drift_before_adoption() {
    mysql::run_mysql_test("owneradopt", |db| async move {
        let identity = ryframe_db::connection::physical_identity(&db).await.map_err(message)?;
        let config = config(&identity.database)?;
        success(run(config.path(), &["control","up"]));
        mysql::execute(&db,"DELETE FROM ryframe_resource_ownership").await?;
        assert!(!run(config.path(), &["control","up"]).status.success());
        let plan = config.path().join("ownership.json");
        let path = plan.to_str().ok_or("计划路径不是UTF8")?;
        mysql::execute(&db,"CREATE TABLE biz_unregistered (id BIGINT PRIMARY KEY)").await?;
        assert!(!run(config.path(), &["control","ownership-adopt","--output",path,"--write"]).status.success());
        assert!(!plan.exists());
        mysql::require_count(&db,"SELECT COUNT(*) AS value FROM ryframe_resource_ownership",0).await?;
        mysql::execute(&db,"DROP TABLE biz_unregistered").await?;
        success(run(config.path(), &["control","ownership-adopt","--output",path,"--write"]));
        mysql::execute(&db,"CREATE VIEW ownership_drift AS SELECT 1 AS id").await?;
        assert!(!run(config.path(), &["control","ownership-adopt","--apply",path,"--write"]).status.success());
        mysql::require_count(&db,"SELECT COUNT(*) AS value FROM ryframe_resource_ownership",0).await?;
        mysql::execute(&db,"DROP VIEW ownership_drift").await?;
        let first = ManagedChild(Some(command(config.path(), &["control","ownership-adopt","--apply",path,"--write"]).spawn().map_err(message)?));
        let second = ManagedChild(Some(command(config.path(), &["control","ownership-adopt","--apply",path,"--write"]).spawn().map_err(message)?));
        success(bounded(first));
        success(bounded(second));
        success(run(config.path(), &["control","ownership-adopt","--inspect",path]));
        success(run(config.path(), &["control","ownership-adopt","--apply",path,"--write"]));
        mysql::require_count(&db,"SELECT COUNT(*) AS value FROM ryframe_resource_ownership",1).await?;
        mysql::execute(&db,"UPDATE ryframe_resource_ownership SET scope_id='other-scope' WHERE resource_kind='control'").await?;
        assert!(!run(config.path(), &["control","up"]).status.success());
        assert!(!run(config.path(), &["control","ownership-adopt","--inspect",path]).status.success());
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_are_not_an_empty_schema_and_partial_tenant_tables_are_not_first_initialization() {
    mysql::run_mysql_test("ownerpartial", |db| async move {
        let identity = ryframe_db::connection::physical_identity(&db).await.map_err(message)?;
        let config = config(&identity.database)?;
        mysql::execute(&db,"CREATE VIEW preexisting_object AS SELECT 1 AS id").await?;
        assert!(!run(config.path(), &["control","up"]).status.success());
        mysql::execute(&db,"DROP VIEW preexisting_object").await?;
        success(run(config.path(), &["control","up"]));
        mysql::execute(&db,ryframe_tenant_db::migration::TENANT_FENCE_DDL).await?;
        assert!(!run(config.path(), &["tenant-data","up","--target","shared-control"]).status.success());
        mysql::require_count(&db,"SELECT COUNT(*) AS value FROM ryframe_resource_ownership WHERE resource_kind='tenant-data'",0).await?;
        Ok(())
    }).await;
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_initialization_releases_both_migration_locks_on_the_original_session() {
    mysql::run_mysql_test("ownercancel", |db| async move {
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let worker_db = db.clone();
        let worker = tokio::spawn(async move {
            let guard = migration_guard::Guard::acquire(&worker_db).await?;
            ryframe_db::connection::physical_identity(guard.session()).await?;
            let _ = ready.send(());
            std::future::pending::<()>().await;
            guard.close().await
        });
        waiting.await.map_err(message)?;
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let guard = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            migration_guard::Guard::acquire(&db),
        )
        .await
        .map_err(message)?
        .map_err(message)?;
        guard.close().await.map_err(message)?;
        mysql::require_count(
            &db,
            "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()",
            0,
        )
        .await?;
        Ok(())
    })
    .await;
}

pub(super) struct ManagedChild(Option<std::process::Child>);

impl ManagedChild {
    pub(super) fn start(directory: &Path, args: &[&str]) -> Result<Self, String> {
        let child = command(directory, args).spawn().map_err(message)?;
        Ok(Self(Some(child)))
    }

    pub(super) fn is_running(&mut self) -> Result<bool, String> {
        Ok(self.0.as_mut().unwrap().try_wait().map_err(message)?.is_none())
    }

    pub(super) fn terminate(&mut self) -> Result<(), String> {
        self.0.as_mut().unwrap().kill().map_err(message)
    }

    pub(super) fn output(self) -> Output {
        bounded(self)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_root_rejects_nested_keeps_atomic_records_and_rejects_failed_commit_reuse() {
    mysql::run_mysql_test("ownerroot", |db| async move {
        let mut guard = migration_guard::Guard::acquire(&db)
            .await
            .map_err(message)?;
        let nested = guard.session().begin().await.map_err(message)?;
        assert!(guard.session_mut().finish_mysql_root(true).await.is_err());
        assert!(
            guard
                .session_mut()
                .begin_mysql_root(None, None)
                .await
                .is_err()
        );
        nested.rollback().await.map_err(message)?;
        guard
            .session_mut()
            .finish_mysql_root(false)
            .await
            .map_err(message)?;
        guard
            .session_mut()
            .begin_mysql_root(None, None)
            .await
            .map_err(message)?;
        mysql::execute(
            guard.session(),
            "CREATE TABLE ownership_session_probe(id INT PRIMARY KEY)",
        )
        .await?;
        // DDL 的隐式提交不能代替下一段登记事务；管理器必须正确回到深度零再开启根事务。
        guard
            .session_mut()
            .finish_mysql_root(true)
            .await
            .map_err(message)?;
        guard
            .session_mut()
            .begin_mysql_root(Some(sea_orm::IsolationLevel::Serializable), None)
            .await
            .map_err(message)?;
        mysql::execute(
            guard.session(),
            "INSERT INTO ownership_session_probe VALUES (1)",
        )
        .await?;
        guard
            .session_mut()
            .finish_mysql_root(true)
            .await
            .map_err(message)?;
        guard
            .session_mut()
            .begin_mysql_root(None, None)
            .await
            .map_err(message)?;
        mysql::execute(
            guard.session(),
            "INSERT INTO ownership_session_probe VALUES (2)",
        )
        .await?;
        guard
            .session_mut()
            .finish_mysql_root(false)
            .await
            .map_err(message)?;
        guard.close().await.map_err(message)?;
        mysql::require_count(
            &db,
            "SELECT COUNT(*) AS value FROM ownership_session_probe",
            1,
        )
        .await?;
        verify_failed_commit_no_replay(&db).await?;
        mysql::require_count(
            &db,
            "SELECT COUNT(*) AS value FROM ownership_session_probe",
            1,
        )
        .await?;
        Ok(())
    })
    .await;
}

async fn verify_failed_commit_no_replay(db: &DatabaseConnection) -> Result<(), String> {
    let mut guard = migration_guard::Guard::acquire(db).await.map_err(message)?;
    mysql::execute(
        guard.session(),
        "INSERT INTO ownership_session_probe VALUES (3)",
    )
    .await?;
    let row = guard
        .session()
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CONNECTION_ID() AS id",
        ))
        .await
        .map_err(message)?
        .ok_or("会话身份缺失")?;
    let id: u64 = row.try_get("", "id").map_err(message)?;
    // 只终止该隔离用例明确持有的 MySQL 会话，故障后不得以同一持有者查询或重放。
    mysql::execute(db, &format!("KILL CONNECTION {id}")).await?;
    assert!(guard.session_mut().finish_mysql_root(true).await.is_err());
    assert!(!guard.session().is_open());
    assert!(
        guard
            .session_mut()
            .begin_mysql_root(None, None)
            .await
            .is_err()
    );
    assert!(
        guard
            .session()
            .query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT 1"))
            .await
            .is_err()
    );
    assert!(guard.close().await.is_err());
    Ok(())
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn bounded(mut child: ManagedChild) -> Output {
    use std::io::Read;
    let process = child.0.as_mut().unwrap();
    let stdout = process.stdout.take().unwrap();
    let stderr = process.stderr.take().unwrap();
    let readers = [
        Box::new(stdout) as Box<dyn Read + Send>,
        Box::new(stderr) as Box<dyn Read + Send>,
    ]
    .map(|mut stream| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        })
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = process.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > std::time::Duration::from_secs(120) {
            let _ = process.kill();
            let _ = process.wait();
            for reader in readers {
                let _ = reader.join();
            }
            panic!("ownership 正式CLI超过120秒；已终止并回收，可能存在同池重租死锁");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    drop(child.0.take());
    let [stdout, stderr] = readers.map(|reader| reader.join().unwrap());
    Output {
        status,
        stdout,
        stderr,
    }
}

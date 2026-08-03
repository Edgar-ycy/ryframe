#![cfg(feature = "file-maintenance")]

use std::{env, error::Error};

use md5::compute as md5_digest;
use ryframe_storage::{ObjectStorage, S3Config, S3ObjectStorage};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const DATABASE_PREFIX: &str = "ryframe_file_a_";
const BUCKET_PREFIX: &str = "ryframe-file-a-";
const MYSQL_PASSWORD: &str = "ryframe_test_password";
const RUSTFS_ACCESS_KEY: &str = "ryframe-test-access";
const RUSTFS_SECRET_KEY: &str = "ryframe-test-secret-2026";
const LEGACY_TENANT: &str = "legacy-fixture";
const FIRST_FILE_ID: i64 = 9_100_001;
const SECOND_FILE_ID: i64 = 9_100_002;
const COLLISION_MD5: &str = "79054025255fb1a26e4bc422aef54eb4";
const FIRST_OBJECT_KEY: &str = "legacy-collision/first.bin";
const SECOND_OBJECT_KEY: &str = "legacy-collision/second.bin";

struct AcceptanceContext {
    database: DatabaseConnection,
    storage: S3ObjectStorage,
    bucket: String,
}

#[tokio::test]
#[ignore = "仅由 scripts/file_a_acceptance.ps1 在隔离容器中运行"]
async fn seed_file_a_legacy_fixture() {
    seed_legacy_fixture()
        .await
        .expect("FILE-A 旧数据与真实对象种子必须成功");
}

#[tokio::test]
#[ignore = "仅由 scripts/file_a_acceptance.ps1 在隔离容器中运行"]
async fn assert_file_a_final_state() {
    assert_final_state()
        .await
        .expect("FILE-A 最终结构与数据断言必须成功");
}

async fn seed_legacy_fixture() -> TestResult {
    let context = acceptance_context().await?;
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.COLUMNS",
            "COLUMN_NAME",
            "file_md5",
        )
        .await?,
        1,
        "种子只能写入仍包含 file_md5 的旧 schema"
    );
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.COLUMNS",
            "COLUMN_NAME",
            "file_sha256",
        )
        .await?,
        0,
        "种子阶段不能提前存在 file_sha256"
    );
    context
        .database
        .execute_unprepared(
            "CREATE INDEX idx_file_upload_reservation \
             ON sys_file (tenant_id, bucket, file_md5, del_flag)",
        )
        .await?;
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.STATISTICS",
            "INDEX_NAME",
            "idx_file_upload_reservation",
        )
        .await?,
        4,
        "种子必须建立待由 000017 删除的旧上传预留索引"
    );

    let first = collision_sample(FIRST_COLLISION_HEX)?;
    let second = collision_sample(SECOND_COLLISION_HEX)?;
    let first_md5 = format!("{:x}", md5_digest(&first));
    let second_md5 = format!("{:x}", md5_digest(&second));
    assert_eq!(first_md5, COLLISION_MD5);
    assert_eq!(second_md5, COLLISION_MD5);
    assert_ne!(first, second, "碰撞样本内容必须不同");

    context.storage.ensure_bucket(&context.bucket).await?;
    context
        .storage
        .put(
            &context.bucket,
            FIRST_OBJECT_KEY,
            &first,
            "application/octet-stream",
        )
        .await?;
    context
        .storage
        .put(
            &context.bucket,
            SECOND_OBJECT_KEY,
            &second,
            "application/octet-stream",
        )
        .await?;
    assert_eq!(
        context
            .storage
            .get(&context.bucket, FIRST_OBJECT_KEY)
            .await?,
        first
    );
    assert_eq!(
        context
            .storage
            .get(&context.bucket, SECOND_OBJECT_KEY)
            .await?,
        second
    );

    insert_legacy_file(
        &context.database,
        &context.bucket,
        FIRST_FILE_ID,
        "first.bin",
        FIRST_OBJECT_KEY,
        first.len(),
    )
    .await?;
    insert_legacy_file(
        &context.database,
        &context.bucket,
        SECOND_FILE_ID,
        "second.bin",
        SECOND_OBJECT_KEY,
        second.len(),
    )
    .await?;

    assert_eq!(
        scalar_i64(
            &context.database,
            "SELECT COUNT(*) FROM sys_file \
             WHERE id IN (9100001, 9100002) \
               AND file_md5 = '79054025255fb1a26e4bc422aef54eb4' \
               AND del_flag = '3'",
        )
        .await?,
        2
    );
    context.database.close().await?;
    println!("FILE-A 旧 schema、MD5 碰撞数据与 RustFS 对象种子已就绪");
    Ok(())
}

async fn assert_final_state() -> TestResult {
    let context = acceptance_context().await?;
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.COLUMNS",
            "COLUMN_NAME",
            "file_md5",
        )
        .await?,
        0,
        "000017 完成后必须删除 file_md5"
    );
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.STATISTICS",
            "INDEX_NAME",
            "idx_file_upload_reservation",
        )
        .await?,
        0,
        "000017 完成后必须删除旧上传预留索引"
    );
    assert_eq!(
        schema_object_count(
            &context.database,
            "information_schema.STATISTICS",
            "INDEX_NAME",
            "idx_file_sha256",
        )
        .await?,
        4,
        "SHA-256 复合索引必须包含四列"
    );
    assert_eq!(
        scalar_i64(
            &context.database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
               AND COLUMN_NAME IN ('file_sha256', 'upload_status', 'del_flag') \
               AND IS_NULLABLE = 'NO'",
        )
        .await?,
        3,
        "最终摘要与状态列必须全部非空"
    );
    assert_eq!(
        scalar_i64(
            &context.database,
            "SELECT COUNT(*) FROM sys_file \
             WHERE id IN (9100001, 9100002) \
               AND upload_status = 'ready' AND del_flag = '0' \
               AND reservation_token IS NULL AND reservation_expires_at IS NULL",
        )
        .await?,
        2,
        "旧上传预留必须全部归一化"
    );

    let first = collision_sample(FIRST_COLLISION_HEX)?;
    let second = collision_sample(SECOND_COLLISION_HEX)?;
    let expected_first_sha256 = hex::encode(Sha256::digest(&first));
    let expected_second_sha256 = hex::encode(Sha256::digest(&second));
    assert_ne!(
        expected_first_sha256, expected_second_sha256,
        "MD5 碰撞样本必须具有不同 SHA-256"
    );
    assert_eq!(
        file_sha256(&context.database, FIRST_FILE_ID).await?,
        expected_first_sha256
    );
    assert_eq!(
        file_sha256(&context.database, SECOND_FILE_ID).await?,
        expected_second_sha256
    );
    assert_eq!(
        context
            .storage
            .get(&context.bucket, FIRST_OBJECT_KEY)
            .await?,
        first,
        "第一个碰撞对象不能被第二个对象误复用"
    );
    assert_eq!(
        context
            .storage
            .get(&context.bucket, SECOND_OBJECT_KEY)
            .await?,
        second,
        "第二个碰撞对象不能被第一个对象误复用"
    );

    context.database.close().await?;
    println!("FILE-A 最终 schema、SHA-256 与碰撞对象隔离断言已通过");
    Ok(())
}

async fn acceptance_context() -> TestResult<AcceptanceContext> {
    let database_name = required_env("FILE_A_DATABASE_NAME")?;
    validate_isolated_name(&database_name, DATABASE_PREFIX, '_', "数据库")?;
    let bucket = required_env("FILE_A_BUCKET")?;
    validate_isolated_name(&bucket, BUCKET_PREFIX, '-', "存储桶")?;

    let mysql_port = loopback_port("FILE_A_MYSQL_PORT")?;
    let rustfs_port = loopback_port("FILE_A_RUSTFS_PORT")?;
    let mysql_password = required_env("FILE_A_MYSQL_PASSWORD")?;
    let rustfs_access_key = required_env("FILE_A_RUSTFS_ACCESS_KEY")?;
    let rustfs_secret_key = required_env("FILE_A_RUSTFS_SECRET_KEY")?;
    if mysql_password != MYSQL_PASSWORD
        || rustfs_access_key != RUSTFS_ACCESS_KEY
        || rustfs_secret_key != RUSTFS_SECRET_KEY
    {
        return Err("FILE-A 夹具拒绝非专用测试凭据".into());
    }

    let database = Database::connect(format!(
        "mysql://root:{mysql_password}@127.0.0.1:{mysql_port}/{database_name}"
    ))
    .await?;
    let actual_database = database
        .query_one_raw(Statement::from_string(
            DatabaseBackend::MySql,
            "SELECT DATABASE() AS database_name".to_owned(),
        ))
        .await?
        .ok_or("无法读取当前数据库名称")?
        .try_get::<String>("", "database_name")?;
    if actual_database != database_name {
        return Err(format!(
            "FILE-A 实际连接数据库不匹配：期望 {database_name}，实际 {actual_database}"
        )
        .into());
    }

    let storage = S3ObjectStorage::new(S3Config {
        endpoint: format!("http://127.0.0.1:{rustfs_port}"),
        access_key: rustfs_access_key,
        secret_key: rustfs_secret_key,
        use_ssl: false,
        region: "us-east-1".to_owned(),
    })?;
    Ok(AcceptanceContext {
        database,
        storage,
        bucket,
    })
}

async fn insert_legacy_file(
    database: &DatabaseConnection,
    bucket: &str,
    id: i64,
    storage_name: &str,
    storage_path: &str,
    file_size: usize,
) -> TestResult {
    database
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO sys_file \
             (id, tenant_id, original_name, storage_name, storage_path, bucket, file_url, \
              file_size, content_type, file_md5, upload_by, del_flag) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'application/octet-stream', ?, 'file-a', '3')",
            [
                id.into(),
                LEGACY_TENANT.into(),
                storage_name.into(),
                storage_name.into(),
                storage_path.into(),
                bucket.into(),
                format!("{bucket}/{storage_path}").into(),
                i64::try_from(file_size)?.into(),
                COLLISION_MD5.into(),
            ],
        ))
        .await?;
    Ok(())
}

async fn file_sha256(database: &DatabaseConnection, id: i64) -> TestResult<String> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT file_sha256 FROM sys_file WHERE id = ?",
            [id.into()],
        ))
        .await?
        .ok_or_else(|| format!("找不到 FILE-A 文件记录 {id}"))?;
    Ok(row.try_get("", "file_sha256")?)
}

async fn schema_object_count(
    database: &DatabaseConnection,
    information_schema_table: &str,
    name_column: &str,
    object_name: &str,
) -> TestResult<i64> {
    if !matches!(
        (information_schema_table, name_column),
        ("information_schema.COLUMNS", "COLUMN_NAME")
            | ("information_schema.STATISTICS", "INDEX_NAME")
    ) {
        return Err("拒绝查询未列入 FILE-A 白名单的元数据对象".into());
    }
    let statement = format!(
        "SELECT COUNT(*) FROM {information_schema_table} \
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' AND {name_column} = ?"
    );
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            statement,
            [object_name.into()],
        ))
        .await?
        .ok_or("元数据计数查询没有返回结果")?;
    Ok(row.try_get_by_index(0)?)
}

async fn scalar_i64(database: &DatabaseConnection, sql: &str) -> TestResult<i64> {
    let row = database
        .query_one_raw(Statement::from_string(
            DatabaseBackend::MySql,
            sql.to_owned(),
        ))
        .await?
        .ok_or("标量查询没有返回结果")?;
    Ok(row.try_get_by_index(0)?)
}

fn required_env(name: &str) -> TestResult<String> {
    let value = env::var(name).map_err(|_| format!("缺少 FILE-A 环境变量 {name}"))?;
    if value.trim().is_empty() {
        return Err(format!("FILE-A 环境变量 {name} 不能为空").into());
    }
    Ok(value)
}

fn loopback_port(name: &str) -> TestResult<u16> {
    let port = required_env(name)?.parse::<u16>()?;
    if port < 1024 {
        return Err(format!("FILE-A 端口 {name} 必须使用非特权端口").into());
    }
    Ok(port)
}

fn validate_isolated_name(value: &str, prefix: &str, separator: char, kind: &str) -> TestResult {
    let suffix = value
        .strip_prefix(prefix)
        .ok_or_else(|| format!("FILE-A {kind}名称缺少隔离前缀 {prefix}"))?;
    if suffix.len() != 12
        || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == separator as u8
        })
    {
        return Err(format!("FILE-A {kind}名称不是 12 位随机隔离名称").into());
    }
    Ok(())
}

fn collision_sample(value: &str) -> TestResult<Vec<u8>> {
    Ok(hex::decode(value.split_whitespace().collect::<String>())?)
}

const FIRST_COLLISION_HEX: &str = "
    d131dd02c5e6eec4693d9a0698aff95c
    2fcab58712467eab4004583eb8fb7f89
    55ad340609f4b30283e488832571415a
    085125e8f7cdc99fd91dbdf280373c5b
    d8823e3156348f5bae6dacd436c919c6
    dd53e2b487da03fd02396306d248cda0
    e99f33420f577ee8ce54b67080a80d1e
    c69821bcb6a8839396f9652b6ff72a70";

const SECOND_COLLISION_HEX: &str = "
    d131dd02c5e6eec4693d9a0698aff95c
    2fcab50712467eab4004583eb8fb7f89
    55ad340609f4b30283e4888325f1415a
    085125e8f7cdc99fd91dbd7280373c5b
    d8823e3156348f5bae6dacd436c919c6
    dd53e23487da03fd02396306d248cda0
    e99f33420f577ee8ce54b67080280d1e
    c69821bcb6a8839396f965ab6ff72a70";

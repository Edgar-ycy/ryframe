use super::{
    bootstrap_hashes, db_error, execute, install_test_database_id_generator, require_count,
    run_mysql_test,
};
use ryframe_application::{
    TransactionAuditMode,
    ports::{
        authorization::AuthorizationMirrorTransaction,
        system::{DictDataRecord, DictTypeRecord},
        tenants::ProvisionTenantRecord,
    },
};
use ryframe_db::{
    AutoFill, ControlDatabaseCluster, FillContext,
    application_ports::{system::dict, transaction::DatabasePortTransaction},
    entities::{cache_namespace_version, dept, menu, permission, role, user},
    migration,
    repositories::{CONFIG_CACHE_NAMESPACE, TenantProvisioningRepository},
};
use ryframe_kernel::AppResult;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    Statement, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, macros::datetime};

#[path = "initialization_data.rs"]
mod data;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_import_and_migrations_produce_equivalent_control_databases() {
    run_mysql_test("sql-init", |imported| async move {
        let sql = migration::mysql_initialization_sql(&bootstrap_hashes());
        imported.execute_unprepared(&sql).await.map_err(db_error)?;
        migration::verify(&imported).await.map_err(db_error)?;
        require_count(
            &imported,
            "SELECT COUNT(*) AS value FROM sys_user WHERE password_change_required = 1",
            2,
        )
        .await?;
        // 已初始化库不再需要初始密码，也不能把已改密账号重新置为必须改密。
        execute(
            &imported,
            "UPDATE sys_user SET password_change_required = 0 WHERE id = 2",
        )
        .await?;
        migration::up(&imported, None).await.map_err(db_error)?;
        require_count(
            &imported,
            "SELECT COUNT(*) AS value FROM sys_user WHERE id = 2 AND password_change_required = 0",
            1,
        )
        .await?;
        execute(
            &imported,
            "UPDATE sys_user SET password_change_required = 1 WHERE id = 2",
        )
        .await?;
        run_mysql_test("migrate-init", move |migrated| async move {
            migration::up(&migrated, Some(&bootstrap_hashes()))
                .await
                .map_err(db_error)?;
            migration::verify(&migrated).await.map_err(db_error)?;
            assert_eq!(inventory(&imported).await?, inventory(&migrated).await?);
            data::compare(&imported, &migrated).await?;
            verify_dictionary_utc_writes(&migrated).await?;
            verify_cache_namespace_utc_writes(&migrated).await?;
            verify_auto_fill_utc_writes(&migrated).await
        })
        .await;
        Ok(())
    })
    .await;
}

async fn verify_cache_namespace_utc_writes(database: &DatabaseConnection) -> Result<(), String> {
    const TENANT: &str = "tenant-cache-utc";
    install_test_database_id_generator()?;
    let transaction = DatabasePortTransaction::from(database.begin().await.map_err(db_error)?);
    verify_cache_namespace_offset(&transaction).await?;
    let record = ProvisionTenantRecord {
        provisioning_request_token: "a".repeat(64),
        tenant_id: TENANT.into(),
        name: "缓存时钟校验租户".into(),
        domain: None,
        expire_at: None,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_minute: 1000,
        admin_username: "cache_utc_admin".into(),
        admin_password_hash: "cache-utc-fixture-hash".into(),
        enabled_capability_route_keys: Vec::new(),
        enabled_capability_permission_codes: Vec::new(),
        managed_capability_route_keys: Vec::new(),
        managed_capability_permission_codes: Vec::new(),
        default_admin_permission_codes: Vec::new(),
    };
    let provisioned = TenantProvisioningRepository
        .initialize_identity_in_transaction(&transaction, &record)
        .await
        .map_err(|error| error.to_string())?;
    let key = (TENANT.to_owned(), CONFIG_CACHE_NAMESPACE.to_owned());
    let initial = cache_namespace_version::Entity::find_by_id(key.clone())
        .one(&transaction)
        .await
        .map_err(db_error)?
        .ok_or("租户开通未写入缓存命名空间初始版本")?;
    assert_eq!(initial.version, 0);
    assert_eq!(initial.created_at, initial.updated_at);
    assert_eq!(initial.created_at.offset(), UtcOffset::UTC);
    let provisioned_second = provisioned.provisioned_at.timestamp();
    assert!(
        (provisioned_second..=provisioned_second + 1)
            .contains(&initial.created_at.unix_timestamp())
    );

    let before = OffsetDateTime::now_utc().unix_timestamp();
    let version = transaction
        .increment_namespace_version(TENANT, CONFIG_CACHE_NAMESPACE)
        .await
        .map_err(|error| error.to_string())?;
    let after = OffsetDateTime::now_utc().unix_timestamp();
    assert_eq!(version, 1);
    transaction.commit().await.map_err(db_error)?;
    let updated = cache_namespace_version::Entity::find_by_id(key)
        .one(database)
        .await
        .map_err(db_error)?
        .ok_or("未找到已递增的缓存命名空间")?;
    assert_eq!(updated.version, 1);
    assert_eq!(updated.created_at, initial.created_at);
    assert_eq!(updated.updated_at.offset(), UtcOffset::UTC);
    assert!((before..=after + 1).contains(&updated.updated_at.unix_timestamp()));
    Ok(())
}

async fn verify_cache_namespace_offset(
    transaction: &DatabasePortTransaction,
) -> Result<(), String> {
    let now = datetime!(2026-08-21 00:59:59.500000001 +08:00);
    cache_namespace_version::ActiveModel {
        tenant_id: ActiveValue::Set("system".into()),
        namespace: ActiveValue::Set("cache-utc-probe".into()),
        version: ActiveValue::Set(0),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(transaction)
    .await
    .map_err(db_error)?;
    // 用同一 DATETIME(0) 列核对驱动传入 UTC 微秒后由 MySQL 应用的秒精度。
    execute(
        transaction,
        "INSERT INTO sys_cache_namespace_version (tenant_id, namespace, version, created_at, updated_at) \
         VALUES ('system', 'cache-utc-reference', 0, \
                 '2026-08-20 16:59:59.500000', '2026-08-20 16:59:59.500000')",
    )
    .await?;
    require_count(
        transaction,
        "SELECT COUNT(*) AS value FROM sys_cache_namespace_version probe \
         JOIN sys_cache_namespace_version baseline \
           ON baseline.tenant_id = probe.tenant_id AND baseline.namespace = 'cache-utc-reference' \
         WHERE probe.tenant_id = 'system' AND probe.namespace = 'cache-utc-probe' \
           AND probe.version = 0 AND probe.created_at = baseline.created_at \
           AND probe.updated_at = baseline.updated_at",
        1,
    )
    .await
}

async fn verify_auto_fill_utc_writes(database: &DatabaseConnection) -> Result<(), String> {
    install_test_database_id_generator()?;
    let create = FillContext {
        now: datetime!(2026-08-21 00:59:59.123456 +08:00),
        user_id: None,
        username: None,
    };
    let update = FillContext {
        now: datetime!(2026-08-20 12:00:00.123456 -05:00),
        user_id: None,
        username: None,
    };
    let stored_created_at = datetime!(2026-08-20 16:59:59 UTC);
    let stored_updated_at = datetime!(2026-08-20 17:00:00 UTC);
    macro_rules! verify_model {
        ($entity:ident, $seed_id:expr, $model:ident, $prepare:block $(, $verify:block)?) => {{
            let mut $model = $entity::Entity::find_by_id($seed_id)
                .one(database)
                .await
                .map_err(db_error)?
                .ok_or(concat!("缺少初始实体 ", stringify!($entity)))?;
            $prepare
            $model.fill_on_insert(&create).map_err(|error| error.to_string())?;
            let id = $model.id;
            assert_ne!(id, $seed_id);
            let mut $model = $entity::ActiveModel::from($model).insert(database).await.map_err(db_error)?;
            assert_eq!($model.created_at, stored_created_at, stringify!($entity));
            assert_eq!($model.updated_at, stored_created_at, stringify!($entity));
            assert_eq!($model.created_at.offset(), UtcOffset::UTC);
            $($verify)?
            $model.fill_on_update(&update).map_err(|error| error.to_string())?;
            assert_eq!($model.id, id);
            let $model = $entity::ActiveModel::from($model)
                .reset_all()
                .update(database)
                .await
                .map_err(db_error)?;
            assert_eq!($model.created_at, stored_created_at, stringify!($entity));
            assert_eq!($model.updated_at, stored_updated_at, stringify!($entity));
            assert_eq!($model.updated_at.offset(), UtcOffset::UTC);
            $($verify)?
        }};
    }
    verify_model!(dept, 1, model, {
        model.name = "填充校验部门".into();
    });
    verify_model!(menu, 1, model, {
        model.name = "填充校验菜单".into();
        model.route_key = None;
    });
    verify_model!(permission, 1, model, {
        model.code = "fill:probe".into();
    });
    verify_model!(role, 2, model, {
        model.code = "fill_probe".into();
    });
    let login = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    verify_model!(
        user,
        2,
        model,
        {
            model.username = "fill_probe".into();
            model.login_date = Some(login);
        },
        {
            assert_eq!(model.login_date, Some(login));
        }
    );
    Ok(())
}

async fn verify_dictionary_utc_writes(database: &DatabaseConnection) -> Result<(), String> {
    let created = datetime!(2026-08-21 00:59:59 +08:00);
    let updated = datetime!(2026-08-20 12:00:00 -05:00);
    write_dictionary(database, created, updated)
        .await
        .map_err(|error| error.to_string())?;
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT t.created_at AS type_created, t.updated_at AS type_updated, \
                    d.created_at AS data_created, d.updated_at AS data_updated \
             FROM sys_dict_type t JOIN sys_dict_data d \
               ON d.tenant_id=t.tenant_id AND d.type_code=t.code \
             WHERE t.tenant_id='system' AND t.id=30041 AND d.id=30042 \
               AND t.name='更新后的类型' AND d.label='更新后的字典项'"
                .to_owned(),
        ))
        .await
        .map_err(db_error)?
        .ok_or("未找到写入并更新的字典类型和字典项")?;
    for (column, expected) in [
        ("type_created", datetime!(2026-08-20 16:59:59)),
        ("data_created", datetime!(2026-08-20 16:59:59)),
        ("type_updated", datetime!(2026-08-20 17:00:00)),
        ("data_updated", datetime!(2026-08-20 17:00:00)),
    ] {
        let actual: PrimitiveDateTime = row.try_get("", column).map_err(db_error)?;
        assert_eq!(actual, expected, "{column}");
    }
    Ok(())
}

async fn write_dictionary(
    database: &DatabaseConnection,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> AppResult<()> {
    const TENANT: &str = "system";
    const TYPE_CODE: &str = "dictionary_utc_probe";
    let persistence = dict(ControlDatabaseCluster::single(database.clone()));
    let transaction = persistence.begin().await?;
    transaction.lock_configuration(TENANT).await?;
    let mut dict_type = transaction
        .insert_type(
            TENANT,
            DictTypeRecord {
                id: 30041,
                name: "原字典类型".into(),
                code: TYPE_CODE.into(),
                status: "1".into(),
                remark: None,
                created_at: created,
                updated_at: created,
            },
        )
        .await?;
    let mut entry = transaction
        .insert_data(
            TENANT,
            DictDataRecord {
                id: 30042,
                type_code: TYPE_CODE.into(),
                label: "原字典项".into(),
                value: "value".into(),
                sort: 0,
                status: "1".into(),
                css_class: None,
                remark: None,
                created_at: created,
                updated_at: created,
            },
        )
        .await?;
    dict_type.name = "更新后的类型".into();
    dict_type.updated_at = updated;
    entry.label = "更新后的字典项".into();
    entry.updated_at = updated;
    transaction.update_type(TENANT, dict_type).await?;
    transaction.update_data(TENANT, entry).await?;
    transaction.increment_configuration_version(TENANT).await?;
    transaction.commit(TransactionAuditMode::Skip).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notice_revision_migration_preserves_rows_from_the_frozen_control_prefix() {
    run_mysql_test("notice-revision-upgrade", |database| async move {
        migration::Migrator::up(&database, Some(3)).await.map_err(db_error)?;
        migration::seed(&database, Some(&bootstrap_hashes())).await.map_err(db_error)?;
        execute(
            &database,
            "INSERT INTO sys_notice(id,title,content) VALUES(34001,'保留标题','保留正文')",
        )
        .await?;

        let before = migration::status(&database).await.map_err(db_error)?;
        assert_eq!(before.missing, ["m20261120_072911_notice_revision"]);
        assert!(before.unexpected.is_empty());
        let missing = migration::verify_current_schema(&database).await.unwrap_err();
        assert!(missing.to_string().contains("missing column sys_notice.revision"));

        migration::up(&database, None).await.map_err(db_error)?;
        migration::verify(&database).await.map_err(db_error)?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_notice WHERE id=34001 AND title='保留标题' AND content='保留正文' AND revision=1",
            1,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_bootstrap_password_does_not_create_tables() {
    run_mysql_test("init-no-secret", |database| async move {
        let error = migration::up(&database, None).await.expect_err("首次初始化必须显式提供密码");
        assert!(error.to_string().contains("[bootstrap]") && error.to_string().contains("admin_password"));
        require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema = DATABASE()", 0).await
    }).await;
}

async fn inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'column', COLUMN_NAME, 'type', COLUMN_TYPE, 'nullable', IS_NULLABLE, 'default', COLUMN_DEFAULT, 'extra', EXTRA, 'collation', COLLATION_NAME) AS CHAR) AS item FROM information_schema.columns WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION",
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'index', INDEX_NAME, 'unique', NON_UNIQUE, 'sequence', SEQ_IN_INDEX, 'column', COLUMN_NAME) AS CHAR) AS item FROM information_schema.statistics WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'column', COLUMN_NAME, 'parent', REFERENCED_TABLE_NAME, 'parent_column', REFERENCED_COLUMN_NAME) AS CHAR) AS item FROM information_schema.key_column_usage WHERE TABLE_SCHEMA = DATABASE() AND REFERENCED_TABLE_NAME IS NOT NULL ORDER BY TABLE_NAME, COLUMN_NAME",
        "SELECT CAST(JSON_OBJECT('tenant', tenant_id, 'username', username, 'status', status, 'change_password', password_change_required) AS CHAR) AS item FROM sys_user ORDER BY tenant_id, username",
        "SELECT CAST(JSON_OBJECT('tenant', tenant_id, 'code', code, 'name', name, 'status', status) AS CHAR) AS item FROM sys_permission ORDER BY tenant_id, code",
        "SELECT CAST(JSON_OBJECT('route', m.route_key, 'name', m.name, 'parent', p.route_key, 'permission', a.code, 'icon', m.icon, 'sort', m.sort) AS CHAR) AS item FROM sys_menu m LEFT JOIN sys_menu p ON m.parent_id = p.id LEFT JOIN sys_permission a ON m.perm_id = a.id ORDER BY m.tenant_id, m.route_key",
    ] {
        let rows = database
            .query_all_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
            .await
            .map_err(db_error)?;
        result.push(
            rows.into_iter()
                .map(|row| row.try_get("", "item").map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(result)
}

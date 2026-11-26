use ryframe_application::{AuthorizationCache, system::identity::ProfileService};
use ryframe_auth::password;
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::{NEW_PASSWORD, administrator, initialize, message, mysql};

const FIRST_AVATAR: i64 = 4001;
const SECOND_AVATAR: i64 = 4002;
const GRACE_SECONDS: i64 = 300;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_login_time_and_avatar_cleanup_preserve_utc() {
    initialize();
    mysql::run_mysql_test("profile-utc", |database| async move {
        let hashes = migration::BootstrapHashes::new(
            password::hash(NEW_PASSWORD).map_err(message)?,
            password::hash("123456").map_err(message)?,
        )
        .map_err(message)?;
        migration::up(&database, Some(&hashes))
            .await
            .map_err(message)?;
        let cache = AuthorizationCache::disabled();
        let profile = ProfileService::new(
            application_ports::users::profile(
                ControlDatabaseCluster::single(database.clone()),
                cache.clone(),
            ),
            cache,
        );
        let actor = administrator();
        assert_eq!(
            profile.get_profile(&actor).await.map_err(message)?.login_date,
            None,
        );
        mysql::execute(
            &database,
            "UPDATE sys_user SET login_date = '2026-08-21 02:03:04' WHERE tenant_id = 'system' AND id = 1",
        )
        .await?;
        assert_eq!(
            profile.get_profile(&actor).await.map_err(message)?.login_date.as_deref(),
            Some("2026-08-21T02:03:04+00:00"),
        );
        mysql::execute(
            &database,
            r#"INSERT INTO sys_file
                (id, tenant_id, original_name, storage_name, storage_path, bucket, file_url,
                 file_size, content_type, file_sha256, upload_by, upload_status)
            VALUES
                (4001, 'system', 'first.png', 'first.png', 'utc/first.png', 'avatar',
                 'avatar/utc/first.png', 1, 'image/png', REPEAT('1', 64), 'admin', 'ready'),
                (4002, 'system', 'second.png', 'second.png', 'utc/second.png', 'avatar',
                 'avatar/utc/second.png', 1, 'image/png', REPEAT('2', 64), 'admin', 'ready')"#,
        )
        .await?;

        let first_url = "avatar/utc/first.png";
        profile
            .update_avatar(&actor, first_url.into(), FIRST_AVATAR)
            .await
            .map_err(message)?;
        profile
            .schedule_unreferenced_avatar_cleanup(&actor, FIRST_AVATAR)
            .await
            .map_err(message)?;
        require_avatar(&database, FIRST_AVATAR, FIRST_AVATAR, "ready", None).await?;
        profile
            .schedule_unreferenced_avatar_cleanup(&actor, SECOND_AVATAR)
            .await
            .map_err(message)?;
        require_avatar(&database, SECOND_AVATAR, FIRST_AVATAR, "cleanup", Some(GRACE_SECONDS)).await?;

        profile
            .update_avatar(&actor, "avatar/utc/second.png".into(), SECOND_AVATAR)
            .await
            .map_err(message)?;
        require_avatar(&database, FIRST_AVATAR, SECOND_AVATAR, "cleanup", Some(GRACE_SECONDS)).await?;
        require_avatar(&database, SECOND_AVATAR, SECOND_AVATAR, "ready", None).await?;
        profile
            .update_avatar(&actor, first_url.into(), FIRST_AVATAR)
            .await
            .map_err(message)?;
        require_avatar(&database, FIRST_AVATAR, FIRST_AVATAR, "ready", None).await?;
        require_avatar(&database, SECOND_AVATAR, FIRST_AVATAR, "cleanup", Some(GRACE_SECONDS)).await
    })
    .await;
}

async fn require_avatar(
    database: &DatabaseConnection,
    file_id: i64,
    expected_reference: i64,
    expected_status: &str,
    expected_grace_seconds: Option<i64>,
) -> Result<(), String> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            r#"SELECT u.avatar_file_id, f.upload_status,
                TIMESTAMPDIFF(SECOND, f.updated_at, f.reservation_expires_at) AS grace_seconds
            FROM sys_file f
            JOIN sys_user u ON u.tenant_id = f.tenant_id AND u.id = 1
            WHERE f.tenant_id = 'system' AND f.id = ?"#,
            [file_id.into()],
        ))
        .await
        .map_err(message)?
        .ok_or_else(|| format!("头像 {file_id} 缺少文件或用户记录"))?;
    let reference: Option<i64> = row.try_get("", "avatar_file_id").map_err(message)?;
    let status: String = row.try_get("", "upload_status").map_err(message)?;
    let grace_seconds: Option<i64> = row.try_get("", "grace_seconds").map_err(message)?;
    assert_eq!(
        reference,
        Some(expected_reference),
        "头像 {file_id} 对应的用户引用"
    );
    assert_eq!(status, expected_status, "头像 {file_id} 的上传状态");
    assert_eq!(
        grace_seconds, expected_grace_seconds,
        "头像 {file_id} 的清理宽限期"
    );
    Ok(())
}

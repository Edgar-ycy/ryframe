use ryframe_db::migration;
use sea_orm::DatabaseConnection;

use super::mysql::{execute, run_mysql_test};

const RESTORE_CHECK: &str = "ck_restore_run_completed";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_schema_checks_are_compared_by_expression_and_enforcement() {
    run_mysql_test("control-checks", |database| async move {
        migration::up(&database)
            .await
            .map_err(|error| format!("安装规范控制库失败: {error}"))?;
        migration::verify_current_schema(&database)
            .await
            .map_err(|error| format!("规范控制库 CHECK 校验失败: {error}"))?;

        replace_restore_check(&database, false).await?;
        require_verification_error(&database, "does not match canonical expression").await?;

        replace_restore_check(&database, true).await?;
        require_verification_error(&database, "enforcement is NOT ENFORCED, expected ENFORCED")
            .await?;

        execute(
            &database,
            "ALTER TABLE `sys_background_job` DROP CHECK `ck_bg_job_attempt_budget`",
        )
        .await?;
        require_verification_error(
            &database,
            "missing check constraint sys_background_job.ck_bg_job_attempt_budget",
        )
        .await
    })
    .await;
}

async fn replace_restore_check(
    database: &DatabaseConnection,
    canonical_but_disabled: bool,
) -> Result<(), String> {
    execute(
        database,
        &format!("ALTER TABLE `sys_restore_run` DROP CHECK `{RESTORE_CHECK}`"),
    )
    .await?;
    let expression = if canonical_but_disabled {
        "((`status` IN ('running', 'data_verified') AND `completed_at` IS NULL) \
         OR (`status` IN ('succeeded', 'failed') AND `completed_at` IS NOT NULL \
         AND `completed_at` >= `started_at`)) NOT ENFORCED"
    } else {
        "((`status` IN ('running', 'data_verified') AND `completed_at` IS NULL) \
         OR (`status` IN ('succeeded', 'failed') AND `completed_at` >= `started_at`))"
    };
    execute(
        database,
        &format!(
            "ALTER TABLE `sys_restore_run` ADD CONSTRAINT `{RESTORE_CHECK}` CHECK {expression}"
        ),
    )
    .await
}

async fn require_verification_error(
    database: &DatabaseConnection,
    expected: &str,
) -> Result<(), String> {
    let error = migration::verify_current_schema(database)
        .await
        .expect_err("被篡改的 CHECK 必须校验失败")
        .to_string();
    if error.contains(expected) {
        Ok(())
    } else {
        Err(format!("CHECK 校验错误缺少 {expected:?}: {error}"))
    }
}

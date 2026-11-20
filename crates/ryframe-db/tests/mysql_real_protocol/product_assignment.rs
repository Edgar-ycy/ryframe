use ryframe_db::repositories::ProductRepository;
use ryframe_kernel::AppError;
use sea_orm::TransactionTrait;

use super::{db_error, execute, run_mysql_test};

#[cfg(feature = "migration")]
#[path = "product_assignment/service.rs"]
mod service;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assignment_reason_update_preserves_assignment_and_requires_current_version() {
    run_mysql_test("assignment-reason", |database| async move {
        execute(
            &database,
            "CREATE TABLE sys_tenant_product_plan (\
                tenant_id VARCHAR(64) NOT NULL PRIMARY KEY, plan_version_id BIGINT NOT NULL,\
                changed_by BIGINT NULL, change_reason VARCHAR(500) NULL,\
                created_at DATETIME(6) NOT NULL, updated_at DATETIME(6) NOT NULL\
            ) ENGINE=InnoDB",
        )
        .await?;
        execute(
            &database,
            "INSERT INTO sys_tenant_product_plan \
             (tenant_id, plan_version_id, changed_by, change_reason, created_at, updated_at) \
             VALUES ('tenant-a', 71, 29, 'Review', '2026-01-01 01:02:03.123456', '2026-01-01 02:03:04.123456')",
        )
        .await?;

        let transaction = database.begin().await.map_err(db_error)?;
        let repository = ProductRepository;
        let original = repository
            .assignment(&transaction, "tenant-a")
            .await
            .map_err(|error| error.to_string())?
            .ok_or("测试租户缺少产品套餐分配")?;
        assert!(matches!(
            repository
                .update_assignment_reason_in_txn(&transaction, "tenant-a", 72, "stale")
                .await,
            Err(AppError::Conflict(_))
        ));
        repository
            .update_assignment_reason_in_txn(&transaction, "tenant-a", 71, "Review")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(
            repository.assignment(&transaction, "tenant-a").await.unwrap().as_ref(),
            Some(&original)
        );
        repository
            .update_assignment_reason_in_txn(&transaction, "tenant-a", 71, "review")
            .await
            .map_err(|error| error.to_string())?;
        transaction.commit().await.map_err(db_error)?;

        let mut expected = original;
        expected.change_reason = Some("review".into());
        assert_eq!(repository.assignment(&database, "tenant-a").await.unwrap(), Some(expected));
        Ok(())
    })
    .await;
}

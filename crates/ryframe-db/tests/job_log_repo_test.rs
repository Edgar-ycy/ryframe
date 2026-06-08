//! JobLogRepository 独立测试
//!
//! 使用 SQLite 内存数据库测试任务日志仓库的 CRUD、过滤查询、清理等功能。

use chrono::{Duration, Utc};
use ryframe_core::auto_fill::{AutoFill, FillContext};
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::JobLogRepository;
use ryframe_db::entities::job_log;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

async fn setup_test_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");
    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");
    db
}

fn make_job_log(
    job_name: &str,
    status: &str,
    start_time: chrono::DateTime<Utc>,
    cost_ms: i64,
) -> job_log::Model {
    let mut model = job_log::Model {
        id: 0,
        job_name: job_name.into(),
        job_group: "DEFAULT".into(),
        message: format!("{}执行完成", job_name),
        status: status.into(),
        error_msg: if status == job_log::Model::STATUS_FAIL {
            Some("执行失败".into())
        } else {
            None
        },
        cost_ms,
        start_time,
    };
    model.fill_on_insert(&FillContext::new());
    model
}

// ==================== CRUD 基础操作 ====================

#[tokio::test]
async fn test_job_log_repo_crud() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    let log = make_job_log(
        "日志清理",
        job_log::Model::STATUS_SUCCESS,
        now(),
        150,
    );
    let inserted = repo.insert(&db, log).await.unwrap();
    assert_eq!(inserted.job_name, "日志清理");
    assert_eq!(inserted.cost_ms, 150);

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.status, job_log::Model::STATUS_SUCCESS);

    // JobLog 支持物理删除
    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_job_log_repo_update_should_fail() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    let log = make_job_log(
        "测试任务",
        job_log::Model::STATUS_SUCCESS,
        now(),
        100,
    );
    let inserted = repo.insert(&db, log).await.unwrap();

    // 任务日志不支持修改
    let result = repo.update(&db, inserted).await;
    assert!(result.is_err());
}

// ==================== 分页 ====================

#[tokio::test]
async fn test_job_log_repo_pagination() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    for i in 0..10 {
        let log = make_job_log(
            &format!("任务{}", i),
            job_log::Model::STATUS_SUCCESS,
            now() - Duration::minutes(i as i64),
            100,
        );
        repo.insert(&db, log).await.unwrap();
    }

    let page = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 5,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 5);
    assert_eq!(page.total, 10);

    let page = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 2,
                page_size: 5,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 5);
}

// ==================== 过滤查询 ====================

#[tokio::test]
async fn test_job_log_repo_find_by_page_filtered() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    let t1 = now() - Duration::hours(2);
    let t2 = now() - Duration::hours(1);
    let t3 = now();

    for (name, status, time) in [
        ("日志清理", job_log::Model::STATUS_SUCCESS, t1),
        ("日志清理", job_log::Model::STATUS_FAIL, t2),
        ("数据备份", job_log::Model::STATUS_SUCCESS, t3),
    ] {
        let log = make_job_log(name, status, time, 100);
        repo.insert(&db, log).await.unwrap();
    }

    // 按任务名称过滤
    let page = repo
        .find_by_page_filtered(
            &db,
            PageQuery::default(),
            Some("日志清理"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 2);

    // 按状态过滤
    let page = repo
        .find_by_page_filtered(
            &db,
            PageQuery::default(),
            None,
            Some(job_log::Model::STATUS_FAIL.to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].job_name, "日志清理");

    // 按时间范围过滤（最近1小时内）
    let page = repo
        .find_by_page_filtered(
            &db,
            PageQuery::default(),
            None,
            None,
            Some(now() - Duration::minutes(30)),
            None,
        )
        .await
        .unwrap();
    assert!(!page.records.is_empty());
}

// ==================== 清理操作 ====================

#[tokio::test]
async fn test_job_log_repo_clean_all() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    for i in 0..5 {
        let log = make_job_log(
            &format!("任务{}", i),
            job_log::Model::STATUS_SUCCESS,
            now(),
            100,
        );
        repo.insert(&db, log).await.unwrap();
    }

    let deleted = repo.clean_all(&db).await.unwrap();
    assert_eq!(deleted, 5);

    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 0);
}

#[tokio::test]
async fn test_job_log_repo_clean_before() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    // 3天前的日志（应被清理）
    for i in 0..3 {
        let log = make_job_log(
            &format!("旧任务{}", i),
            job_log::Model::STATUS_SUCCESS,
            now() - Duration::days(5),
            100,
        );
        repo.insert(&db, log).await.unwrap();
    }

    // 最近的日志（不应被清理）
    for i in 0..2 {
        let log = make_job_log(
            &format!("新任务{}", i),
            job_log::Model::STATUS_SUCCESS,
            now(),
            100,
        );
        repo.insert(&db, log).await.unwrap();
    }

    // 清理3天前的日志
    let deleted = repo.clean_before(&db, 3).await.unwrap();
    assert_eq!(deleted, 3);

    // 剩余的应该是2条
    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 2);
}

#[tokio::test]
async fn test_job_log_repo_clean_before_none() {
    let db = setup_test_db().await;
    let repo = JobLogRepository;

    for i in 0..3 {
        let log = make_job_log(
            &format!("任务{}", i),
            job_log::Model::STATUS_SUCCESS,
            now(),
            100,
        );
        repo.insert(&db, log).await.unwrap();
    }

    // 清理365天前的日志 -- 不应该删除任何日志
    let deleted = repo.clean_before(&db, 365).await.unwrap();
    assert_eq!(deleted, 0);

    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 3);
}

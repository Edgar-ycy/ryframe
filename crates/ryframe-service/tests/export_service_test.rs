mod common;

use std::sync::Arc;
use std::time::Duration;

use ryframe_config::JobConfig;
use ryframe_db::{
    DatabaseCluster,
    entities::{background_job, export_job, role, user, user_role},
};
use ryframe_kernel::{ActorContext, DataScope};
use ryframe_service::{
    ExportJobHandler, JobQueue, JobRunResult, JobWorker,
    system::{ExportService, RequestExportCommand, UserExportFilters, UserService},
};
use sea_orm::{ConnectionTrait, EntityTrait, Schema, Set};
use serde_json::to_value;
use tempfile::tempdir;

fn actor() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "system".into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

async fn seed_export_requester(database: &sea_orm::DatabaseConnection) {
    let now = chrono::Utc::now();
    user::Entity::insert(user::ActiveModel {
        id: Set(1),
        tenant_id: Set("system".into()),
        username: Set("admin".into()),
        password_hash: Set("not-used-in-export-test".into()),
        nickname: Set("管理员".into()),
        email: Set("admin@example.com".into()),
        phone: Set("13800000000".into()),
        avatar: Set(None),
        avatar_file_id: Set(None),
        preferred_locale: Set(None),
        status: Set(user::Model::STATUS_NORMAL.into()),
        authorization_version: Set(1),
        dept_id: Set(None),
        remark: Set(None),
        login_ip: Set(None),
        login_date: Set(None),
        del_flag: Set(user::Model::DEL_FLAG_NORMAL.into()),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .exec(database)
    .await
    .unwrap();
    role::Entity::insert(role::ActiveModel {
        id: Set(1),
        tenant_id: Set("system".into()),
        name: Set("超级管理员".into()),
        code: Set("super_admin".into()),
        is_super: Set(1),
        data_scope: Set(role::Model::DATA_SCOPE_ALL.into()),
        status: Set(role::Model::STATUS_NORMAL.into()),
        sort: Set(0),
        remark: Set(None),
        del_flag: Set(role::Model::DEL_FLAG_NORMAL.into()),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .exec(database)
    .await
    .unwrap();
    user_role::Entity::insert(user_role::ActiveModel {
        tenant_id: Set("system".into()),
        user_id: Set(1),
        role_id: Set(1),
    })
    .exec(database)
    .await
    .unwrap();
}

/// 数据库时钟、并行测试连接与新建任务可见性之间可能存在短暂延迟，轮询模拟真实 Worker 行为。
async fn run_until_claimed(worker: &JobWorker, worker_id: &str) -> JobRunResult {
    for _ in 0..50 {
        let result = worker.run_once(worker_id).await.unwrap();
        if result != JobRunResult::Idle {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    JobRunResult::Idle
}

/// 导出任务必须在 Worker 成功后才暴露文件结果，且结果文件属于申请人所在租户。
#[tokio::test]
async fn user_export_is_created_processed_and_exposed_as_a_result_file() {
    let database = common::setup_test_db().await;
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(background_job::Entity))
        .await
        .unwrap();
    database
        .execute(&schema.create_table_from_entity(export_job::Entity))
        .await
        .unwrap();
    seed_export_requester(database.connection()).await;
    let directory = tempdir().unwrap();
    let cluster = DatabaseCluster::single(database.connection().clone());
    let users = Arc::new(UserService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    ));
    let exports = Arc::new(ExportService::new(
        cluster.clone(),
        users,
        Arc::new(ryframe_storage::LocalObjectStorage::new(directory.path())),
        &JobConfig::default(),
    ));
    let queue = Arc::new(JobQueue::new(cluster));

    let requested = exports
        .request(
            &actor(),
            RequestExportCommand {
                resource: "users".into(),
                permission_code: "system:user:export".into(),
                request_params: to_value(UserExportFilters {
                    username: None,
                    phone: None,
                    status: None,
                    dept_id: None,
                })
                .unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(requested.status, "queued");

    let worker = JobWorker::new(queue, &JobConfig::default())
        .unwrap()
        .with_handler(Arc::new(ExportJobHandler::new(exports.clone())))
        .unwrap();
    assert_eq!(
        run_until_claimed(&worker, "export-test-worker").await,
        JobRunResult::Succeeded
    );

    let completed = exports
        .find_for_requester(&actor(), requested.id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        completed.status, "succeeded",
        "导出失败原因：{:?}",
        completed.error_message
    );
    assert!(completed.result_file_name.is_some());
    assert!(completed.expires_at.is_some());

    database
        .execute_unprepared(
            "UPDATE sys_export_job SET expires_at = UTC_TIMESTAMP() - INTERVAL 1 SECOND",
        )
        .await
        .unwrap();
    assert_eq!(exports.cleanup_expired().await.unwrap(), 1);
    let expired = export_job::Entity::find_by_id(requested.id.parse::<i64>().unwrap())
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.status, export_job::Model::STATUS_EXPIRED);
}

/// Worker 必须以数据库当前角色重新校验权限，不能信任任务创建时的权限快照。
#[tokio::test]
async fn revoked_export_permission_stops_queued_job_without_retrying() {
    let database = common::setup_test_db().await;
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(background_job::Entity))
        .await
        .unwrap();
    database
        .execute(&schema.create_table_from_entity(export_job::Entity))
        .await
        .unwrap();
    seed_export_requester(database.connection()).await;
    let directory = tempdir().unwrap();
    let cluster = DatabaseCluster::single(database.connection().clone());
    let users = Arc::new(UserService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    ));
    let exports = Arc::new(ExportService::new(
        cluster.clone(),
        users,
        Arc::new(ryframe_storage::LocalObjectStorage::new(directory.path())),
        &JobConfig::default(),
    ));
    let queue = Arc::new(JobQueue::new(cluster));

    let requested = exports
        .request(
            &actor(),
            RequestExportCommand {
                resource: "users".into(),
                permission_code: "system:user:export".into(),
                request_params: to_value(UserExportFilters {
                    username: None,
                    phone: None,
                    status: None,
                    dept_id: None,
                })
                .unwrap(),
            },
        )
        .await
        .unwrap();
    database
        .execute_unprepared("UPDATE sys_role SET status = '0' WHERE id = 1")
        .await
        .unwrap();

    let worker = JobWorker::new(queue, &JobConfig::default())
        .unwrap()
        .with_handler(Arc::new(ExportJobHandler::new(exports.clone())))
        .unwrap();
    assert_eq!(
        run_until_claimed(&worker, "export-revoked-worker").await,
        JobRunResult::Succeeded
    );

    let completed = export_job::Entity::find_by_id(requested.id.parse::<i64>().unwrap())
        .one(database.connection())
        .await
        .unwrap()
        .expect("已撤权的导出任务仍应保留失败记录");
    assert_eq!(completed.status, export_job::Model::STATUS_FAILED);
    assert!(
        completed
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("权限已被撤销"))
    );
}

/// 每种导出资源都必须能由 Worker 正确分派、生成受控结果文件并完成任务。
#[tokio::test]
async fn worker_processes_every_supported_export_resource() {
    let database = common::setup_test_db().await;
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(background_job::Entity))
        .await
        .unwrap();
    database
        .execute(&schema.create_table_from_entity(export_job::Entity))
        .await
        .unwrap();
    seed_export_requester(database.connection()).await;
    let directory = tempdir().unwrap();
    let cluster = DatabaseCluster::single(database.connection().clone());
    let users = Arc::new(UserService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    ));
    let exports = Arc::new(ExportService::new(
        cluster.clone(),
        users,
        Arc::new(ryframe_storage::LocalObjectStorage::new(directory.path())),
        &JobConfig::default(),
    ));
    let queue = Arc::new(JobQueue::new(cluster));
    let worker = JobWorker::new(queue, &JobConfig::default())
        .unwrap()
        .with_handler(Arc::new(ExportJobHandler::new(exports.clone())))
        .unwrap();

    for (resource, permission_code) in [
        ("users", "system:user:export"),
        ("roles", "system:role:export"),
        ("posts", "system:post:export"),
        ("configs", "system:config:export"),
        ("dict-types", "system:dict:export"),
        ("operlogs", "system:operlog:export"),
        ("loginlogs", "system:logininfor:export"),
    ] {
        let requested = exports
            .request(
                &actor(),
                RequestExportCommand {
                    resource: resource.into(),
                    permission_code: permission_code.into(),
                    request_params: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            run_until_claimed(&worker, &format!("{resource}-export-worker")).await,
            JobRunResult::Succeeded,
            "资源 {resource} 未被 Worker 成功处理"
        );
        let completed = exports
            .find_for_requester(&actor(), requested.id.parse().unwrap())
            .await
            .unwrap();
        assert_eq!(
            completed.status,
            export_job::Model::STATUS_SUCCEEDED,
            "资源 {resource} 导出失败：{:?}",
            completed.error_message
        );
        assert!(
            completed
                .result_file_name
                .as_deref()
                .is_some_and(|file_name| file_name.starts_with(resource)),
            "资源 {resource} 未生成对应的结果文件"
        );
    }
}

/// 导出查询必须跨越单批大小后仍以主键严格递增，避免页码偏移导致的重复或遗漏。
#[tokio::test]
async fn user_export_query_reads_more_than_one_cursor_batch_in_primary_key_order() {
    let database = common::setup_test_db().await;
    seed_export_requester(database.connection()).await;
    let now = chrono::Utc::now();
    let users = (2_i64..=1_002)
        .map(|id| user::ActiveModel {
            id: Set(id),
            tenant_id: Set("system".into()),
            username: Set(format!("export-user-{id}")),
            password_hash: Set("not-used-in-export-test".into()),
            nickname: Set(format!("导出用户{id}")),
            email: Set(format!("export-user-{id}@example.com")),
            phone: Set(format!("138{id:08}")),
            avatar: Set(None),
            avatar_file_id: Set(None),
            preferred_locale: Set(None),
            status: Set(user::Model::STATUS_NORMAL.into()),
            authorization_version: Set(1),
            dept_id: Set(None),
            remark: Set(None),
            login_ip: Set(None),
            login_date: Set(None),
            del_flag: Set(user::Model::DEL_FLAG_NORMAL.into()),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .collect::<Vec<_>>();
    user::Entity::insert_many(users)
        .exec(database.connection())
        .await
        .unwrap();

    let service = UserService::new(
        DatabaseCluster::single(database.connection().clone()),
        ryframe_service::AuthorizationCache::disabled(),
    );
    let exported = service
        .find_for_export(&actor(), None, None, None, None, 500_000)
        .await
        .unwrap();

    assert_eq!(exported.len(), 1_002);
    let ids = exported
        .iter()
        .map(|user| user.id.parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(ids.first(), Some(&1));
    assert_eq!(ids.last(), Some(&1_002));
}

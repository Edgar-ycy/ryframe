mod common;

use std::time::Duration;

use common::setup_test_db;
use ryframe_common::{ActorContext, AppError, DataScope};
use ryframe_core::Repository;
use ryframe_db::{
    DatabaseCluster, RoleRepository, UserRepository,
    entities::{role, user},
};
use ryframe_service::system::{UpdateUserParams, UserService};
use sea_orm::{ActiveModelTrait, DatabaseConnection, TransactionTrait};

const TENANT_ID: &str = "system";

fn actor() -> ActorContext {
    ActorContext {
        user_id: 99,
        tenant_id: TENANT_ID.into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn user_service(db: &DatabaseConnection) -> UserService {
    UserService::new(DatabaseCluster::single(db.clone()), None)
}

async fn seed_user(db: &DatabaseConnection, id: i64, username: &str) {
    let now = chrono::Utc::now();
    let model = user::Model {
        id,
        tenant_id: TENANT_ID.into(),
        username: username.into(),
        password_hash: "test-only-hash".into(),
        nickname: format!("Original {username}"),
        email: format!("{username}@example.test"),
        phone: String::new(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.into(),
        auth_version: 1,
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    };
    let active: user::ActiveModel = model.into();
    active.insert(db).await.expect("seed user");
}

async fn seed_super_role(db: &DatabaseConnection) -> i64 {
    let now = chrono::Utc::now();
    let model = role::Model {
        id: 90_001,
        tenant_id: TENANT_ID.into(),
        name: "Super Role".into(),
        code: "concurrency-super".into(),
        is_super: 1,
        data_scope: role::Model::DATA_SCOPE_ALL.into(),
        status: role::Model::STATUS_NORMAL.into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    };
    let id = model.id;
    let active: role::ActiveModel = model.into();
    active.insert(db).await.expect("seed super role");
    id
}

#[tokio::test]
async fn concurrent_super_promotion_blocks_every_user_mutation_after_lock_wait() {
    let db = setup_test_db().await;
    let target_ids = [10_001, 10_002, 10_003, 10_004, 10_006];
    let batch_normal_id = 10_005;
    for (index, id) in target_ids.into_iter().enumerate() {
        seed_user(db.connection(), id, &format!("target_{index}")).await;
    }
    seed_user(db.connection(), batch_normal_id, "batch_normal").await;
    let super_role_id = seed_super_role(db.connection()).await;

    // Stage promotions while retaining each target user's FOR UPDATE lock.
    // Every service operation below must wait, then re-read the committed role
    // membership instead of proceeding with its earlier non-super observation.
    let promotion = db.begin().await.expect("begin promotion transaction");
    for id in target_ids {
        RoleRepository
            .replace_roles_in_txn(&promotion, TENANT_ID, id, &[super_role_id])
            .await
            .expect("stage super-admin promotion");
    }

    let actor = actor();
    let stale_precheck_service = user_service(db.connection());
    for id in target_ids {
        assert!(
            !stale_precheck_service
                .is_super_admin_user(&actor, id)
                .await
                .expect("old non-locking precheck remains readable"),
            "an uncommitted promotion must reproduce the stale precheck window"
        );
    }

    let replace_service = user_service(db.connection());
    let replace_actor = actor.clone();
    let mut replace_task = tokio::spawn(async move {
        replace_service
            .replace_roles(&replace_actor, 10_001, Vec::new())
            .await
    });

    let update_service = user_service(db.connection());
    let update_actor = actor.clone();
    let mut update_task = tokio::spawn(async move {
        update_service
            .update(
                &update_actor,
                UpdateUserParams {
                    id: 10_002,
                    nickname: "Concurrent Update",
                    email: "updated@example.test",
                    phone: "13800000000",
                    dept_id: None,
                },
            )
            .await
            .map(|_| ())
    });

    let status_service = user_service(db.connection());
    let status_actor = actor.clone();
    let mut status_task = tokio::spawn(async move {
        status_service
            .update_status(&status_actor, 10_003, user::Model::STATUS_DISABLED.into())
            .await
    });

    let delete_service = user_service(db.connection());
    let delete_actor = actor.clone();
    let mut delete_task =
        tokio::spawn(async move { delete_service.delete(&delete_actor, 10_004).await });

    let batch_service = user_service(db.connection());
    let batch_actor = actor.clone();
    let mut batch_task = tokio::spawn(async move {
        batch_service
            .delete_many(&batch_actor, &[10_006, batch_normal_id])
            .await
            .map(|_| ())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!replace_task.is_finished(), "role replacement must wait");
    assert!(!update_task.is_finished(), "user update must wait");
    assert!(!status_task.is_finished(), "status update must wait");
    assert!(!delete_task.is_finished(), "single delete must wait");
    assert!(!batch_task.is_finished(), "batch delete must wait");

    promotion.commit().await.expect("commit promotions");

    for result in [
        (&mut replace_task).await.expect("replace task joins"),
        (&mut update_task).await.expect("update task joins"),
        (&mut status_task).await.expect("status task joins"),
        (&mut delete_task).await.expect("delete task joins"),
        (&mut batch_task).await.expect("batch task joins"),
    ] {
        assert!(matches!(result, Err(AppError::Authorization(_))));
    }

    let repository = UserRepository;
    for id in [10_001, 10_002, 10_003, 10_004, 10_006, batch_normal_id] {
        let saved = repository
            .find_by_id(db.connection(), TENANT_ID, id)
            .await
            .expect("load protected user")
            .expect("rejected mutation must not delete user");
        assert_eq!(saved.auth_version, 1, "rejected mutation must roll back");
        assert_eq!(saved.status, user::Model::STATUS_NORMAL);
        assert_ne!(saved.nickname, "Concurrent Update");
    }

    let roles = RoleRepository
        .find_user_roles_all_status(db.connection(), TENANT_ID, 10_001)
        .await
        .expect("load roles after rejected replacement");
    assert!(roles.iter().any(|role| role.is_super == 1));
}

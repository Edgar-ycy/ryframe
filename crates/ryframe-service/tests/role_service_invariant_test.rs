mod common;

use std::{sync::Arc, time::Duration};

use common::setup_test_db;
use ryframe_common::{ActorContext, AppError, DataScope};
use ryframe_core::Repository;
use ryframe_db::{
    DatabaseCluster, PermissionRepository, RoleRepository,
    entities::{permission, role},
};
use ryframe_service::system::RoleService;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
    sea_query::Expr,
};
use tokio::sync::Barrier;

const TENANT_ID: &str = "system";

fn actor() -> ActorContext {
    ActorContext {
        user_id: 99,
        tenant_id: TENANT_ID.into(),
        username: "platform-admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn role_service(db: &DatabaseConnection) -> RoleService {
    RoleService::new(DatabaseCluster::single(db.clone()), None)
}

async fn seed_role(db: &DatabaseConnection, id: i64, code: &str, is_super: i8) -> role::Model {
    let now = chrono::Utc::now();
    let model = role::Model {
        id,
        tenant_id: TENANT_ID.into(),
        name: code.into(),
        code: code.into(),
        is_super,
        data_scope: role::Model::DATA_SCOPE_ALL.into(),
        status: role::Model::STATUS_NORMAL.into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    };
    let active: role::ActiveModel = model.clone().into();
    active.insert(db).await.expect("seed role");
    model
}

async fn seed_permission(db: &DatabaseConnection, id: i64, code: &str) -> permission::Model {
    let now = chrono::Utc::now();
    let model = permission::Model {
        id,
        tenant_id: TENANT_ID.into(),
        name: code.into(),
        code: code.into(),
        parent_id: None,
        perm_type: "api".into(),
        icon: None,
        sort: 0,
        status: "1".into(),
        created_at: now,
        updated_at: now,
    };
    let active: permission::ActiveModel = model.clone().into();
    active.insert(db).await.expect("seed permission");
    model
}

#[tokio::test]
async fn last_available_super_role_cannot_be_disabled_or_deleted() {
    let db = setup_test_db().await;
    let service = role_service(db.connection());
    let actor = actor();
    let super_role = seed_role(db.connection(), 41_001, "only-super", 1).await;
    let ordinary_role = seed_role(db.connection(), 41_002, "ordinary", 0).await;

    let disable_error = service
        .update(
            &actor,
            super_role.id,
            "Only Super",
            1,
            role::Model::STATUS_DISABLED.into(),
            None,
        )
        .await
        .expect_err("the last usable super role must not be disabled");
    assert!(matches!(disable_error, AppError::Conflict(_)));

    let delete_error = service
        .delete(&actor, super_role.id)
        .await
        .expect_err("the last usable super role must not be deleted");
    assert!(matches!(delete_error, AppError::Conflict(_)));

    let batch_error = service
        .delete_many(&actor, &[ordinary_role.id, super_role.id])
        .await
        .expect_err("a batch containing the last super role must be atomic");
    assert!(matches!(batch_error, AppError::Conflict(_)));
    assert!(
        RoleRepository
            .find_by_id(db.connection(), TENANT_ID, ordinary_role.id)
            .await
            .unwrap()
            .is_some(),
        "the ordinary role must roll back with the rejected batch"
    );

    let edited = service
        .update(
            &actor,
            super_role.id,
            "Renamed Super",
            2,
            role::Model::STATUS_NORMAL.into(),
            None,
        )
        .await
        .expect("non-destructive edits of a super role remain allowed");
    assert_eq!(edited.name, "Renamed Super");

    let backup = seed_role(db.connection(), 41_003, "backup-super", 1).await;
    service
        .update(
            &actor,
            super_role.id,
            "Renamed Super",
            2,
            role::Model::STATUS_DISABLED.into(),
            None,
        )
        .await
        .expect("one super role may be disabled when another remains usable");
    let backup_delete_error = service
        .delete(&actor, backup.id)
        .await
        .expect_err("the remaining usable super role must stay protected");
    assert!(matches!(backup_delete_error, AppError::Conflict(_)));
}

#[tokio::test]
async fn concurrent_super_role_deletes_leave_exactly_one_available() {
    let db = setup_test_db().await;
    let first = seed_role(db.connection(), 42_001, "concurrent-super-a", 1).await;
    let second = seed_role(db.connection(), 42_002, "concurrent-super-b", 1).await;
    let first_id = first.id;
    let second_id = second.id;
    let barrier = Arc::new(Barrier::new(3));

    let first_db = db.connection().clone();
    let first_actor = actor();
    let first_barrier = barrier.clone();
    let first_delete = tokio::spawn(async move {
        first_barrier.wait().await;
        role_service(&first_db).delete(&first_actor, first_id).await
    });

    let second_db = db.connection().clone();
    let second_actor = actor();
    let second_barrier = barrier.clone();
    let second_delete = tokio::spawn(async move {
        second_barrier.wait().await;
        role_service(&second_db)
            .delete(&second_actor, second_id)
            .await
    });

    barrier.wait().await;
    let (first_result, second_result) = tokio::join!(first_delete, second_delete);
    let results = [
        first_result.expect("first delete task joins"),
        second_result.expect("second delete task joins"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(AppError::Conflict(_))))
            .count(),
        1
    );

    let available = RoleRepository
        .find_by_ids(db.connection(), TENANT_ID, &[first_id, second_id])
        .await
        .expect("load remaining roles");
    assert_eq!(available.len(), 1);
    assert_eq!(available[0].is_super, 1);
    assert_eq!(available[0].status, role::Model::STATUS_NORMAL);
}

#[tokio::test]
async fn concurrent_promotion_is_rechecked_after_the_role_row_lock() {
    let db = setup_test_db().await;
    let target = seed_role(db.connection(), 43_002, "promoted-super", 0).await;
    let ordinary = seed_role(db.connection(), 43_001, "batch-companion", 0).await;
    let target_id = target.id;
    let ordinary_id = ordinary.id;

    let promotion = db.begin().await.expect("begin promotion transaction");
    role::Entity::update_many()
        .col_expr(role::Column::IsSuper, Expr::value(1))
        .filter(role::Column::Id.eq(target_id))
        .filter(role::Column::TenantId.eq(TENANT_ID))
        .exec(&promotion)
        .await
        .expect("stage super-role promotion");

    let stale = RoleRepository
        .find_by_id(db.connection(), TENANT_ID, target_id)
        .await
        .expect("read old role state")
        .expect("target exists");
    assert_eq!(
        stale.is_super, 0,
        "the old precheck must reproduce the race"
    );

    let update_db = db.connection().clone();
    let update_actor = actor();
    let mut update = tokio::spawn(async move {
        role_service(&update_db)
            .update(
                &update_actor,
                target_id,
                "Stale Update",
                0,
                role::Model::STATUS_DISABLED.into(),
                None,
            )
            .await
            .map(|_| ())
    });

    let delete_db = db.connection().clone();
    let delete_actor = actor();
    let mut delete = tokio::spawn(async move {
        role_service(&delete_db)
            .delete(&delete_actor, target_id)
            .await
    });

    let batch_db = db.connection().clone();
    let batch_actor = actor();
    let mut batch = tokio::spawn(async move {
        role_service(&batch_db)
            .delete_many(&batch_actor, &[target_id, ordinary_id])
            .await
            .map(|_| ())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!update.is_finished(), "update must wait for the role lock");
    assert!(
        !delete.is_finished(),
        "delete must wait for the mutation lock order"
    );
    assert!(
        !batch.is_finished(),
        "batch delete must wait for the mutation lock order"
    );

    promotion.commit().await.expect("commit role promotion");
    for result in [
        (&mut update).await.expect("update task joins"),
        (&mut delete).await.expect("delete task joins"),
        (&mut batch).await.expect("batch task joins"),
    ] {
        assert!(matches!(result, Err(AppError::Conflict(_))));
    }

    let protected = RoleRepository
        .find_by_id(db.connection(), TENANT_ID, target_id)
        .await
        .expect("load protected role")
        .expect("promoted role remains live");
    assert_eq!(protected.is_super, 1);
    assert_eq!(protected.status, role::Model::STATUS_NORMAL);
    assert!(
        RoleRepository
            .find_by_id(db.connection(), TENANT_ID, ordinary_id)
            .await
            .expect("load batch companion")
            .is_some(),
        "the rejected batch must not partially delete its ordinary role"
    );
}

#[tokio::test]
async fn duplicate_permission_ids_are_normalized_before_replacement() {
    let db = setup_test_db().await;
    let target = seed_role(db.connection(), 44_001, "permission-target", 0).await;
    let first = seed_permission(db.connection(), 44_101, "role:duplicate:first").await;
    let second = seed_permission(db.connection(), 44_102, "role:duplicate:second").await;

    role_service(db.connection())
        .assign_permissions(
            &actor(),
            target.id,
            vec![second.id, first.id, second.id, first.id],
        )
        .await
        .expect("duplicate permission IDs must not reach insert_many");

    let mut saved = PermissionRepository
        .find_role_perm_ids(db.connection(), TENANT_ID, target.id)
        .await
        .expect("load assigned permissions");
    saved.sort_unstable();
    assert_eq!(saved, vec![first.id, second.id]);
}

mod common;

use common::setup_test_db;
use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, DataScope};
use ryframe_core::Repository;
use ryframe_db::{
    DatabaseCluster, PasswordResetRequestRepository, RoleRepository, UserRepository,
    entities::{password_reset_request, role, user},
};
use ryframe_service::system::{CreateUserParams, UserService};
use sea_orm::{ActiveModelTrait, TransactionTrait};

fn user_service(db: &sea_orm::DatabaseConnection) -> UserService {
    UserService::new(DatabaseCluster::single(db.clone()), None)
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 99,
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

async fn create_target_user(svc: &UserService, actor: &ActorContext) -> i64 {
    let user = svc
        .create(
            actor,
            CreateUserParams {
                username: "reset_target",
                nickname: "Reset Target",
                email: "reset_target@test.com",
                phone: "13800000000",
                dept_id: None,
                role_ids: Vec::new(),
            },
        )
        .await
        .expect("create target user");

    user.id.parse().expect("snowflake id should parse")
}

#[tokio::test]
async fn test_request_password_reset_persists_pending_request() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;

    let outcome = svc
        .request_password_reset(
            &actor,
            target_user_id,
            "  forgot password  ",
            Some("127.0.0.1".to_string()),
        )
        .await
        .expect("request password reset");
    let request = outcome.request;

    assert_eq!(request.target_user_id, target_user_id);
    assert_eq!(request.requested_by, actor.user_id);
    assert_eq!(request.reason, "forgot password");
    assert_eq!(
        request.status,
        password_reset_request::Model::STATUS_PENDING
    );
    assert_eq!(request.request_ip.as_deref(), Some("127.0.0.1"));
    assert!(request.completed_at.is_none());
    assert!(request.expires_at > chrono::Utc::now());
    assert!(!request.token_hash.is_empty());
    assert!(!outcome.token.is_empty());
    assert!(password::verify(&outcome.token, &request.token_hash).unwrap());

    let pending = PasswordResetRequestRepository
        .find_pending_by_target(&db, &actor.tenant_id, target_user_id)
        .await
        .expect("find pending reset requests");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, request.id);
}

#[tokio::test]
async fn test_request_password_reset_requires_reason() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;

    let err = svc
        .request_password_reset(&actor, target_user_id, "   ", None)
        .await
        .expect_err("blank reason should be rejected");

    assert!(matches!(err, AppError::Validation(_)));
}

#[tokio::test]
async fn test_complete_password_reset_sets_new_password_and_activates_pending_user() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;

    let outcome = svc
        .request_password_reset(&actor, target_user_id, "activate account", None)
        .await
        .expect("request password reset");

    let err = svc
        .complete_password_reset_request(
            &actor.tenant_id,
            outcome.request.id,
            &outcome.token,
            "newpass123",
        )
        .await
        .expect_err("weak passwords must always be rejected");
    assert!(matches!(err, AppError::Validation(_)));

    let completed_user_id = svc
        .complete_password_reset_request(
            &actor.tenant_id,
            outcome.request.id,
            &outcome.token,
            "NewPass123!",
        )
        .await
        .expect("complete password reset");

    assert_eq!(completed_user_id, target_user_id);

    let saved_user = UserRepository
        .find_by_id(&db, &actor.tenant_id, target_user_id)
        .await
        .expect("find user")
        .expect("user exists");
    assert_eq!(saved_user.status, user::Model::STATUS_NORMAL);
    assert!(password::verify("NewPass123!", &saved_user.password_hash).unwrap());

    let saved_request = PasswordResetRequestRepository
        .find_by_id(&db, &actor.tenant_id, outcome.request.id)
        .await
        .expect("find reset request")
        .expect("reset request exists");
    assert_eq!(
        saved_request.status,
        password_reset_request::Model::STATUS_COMPLETED
    );
    assert!(saved_request.completed_at.is_some());
}

#[tokio::test]
async fn test_password_reset_request_can_only_be_consumed_once() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;
    let outcome = svc
        .request_password_reset(&actor, target_user_id, "concurrent reset", None)
        .await
        .expect("request password reset");

    let first_password = "ConcurrentOne123!";
    let second_password = "ConcurrentTwo123!";
    let (first, second) = tokio::join!(
        svc.complete_password_reset_request(
            &actor.tenant_id,
            outcome.request.id,
            &outcome.token,
            first_password,
        ),
        svc.complete_password_reset_request(
            &actor.tenant_id,
            outcome.request.id,
            &outcome.token,
            second_password,
        ),
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let winning_password = if first.is_ok() {
        first_password
    } else {
        second_password
    };
    let saved_user = UserRepository
        .find_by_id(&db, &actor.tenant_id, target_user_id)
        .await
        .expect("find user")
        .expect("user exists");
    assert_eq!(saved_user.auth_version, 2);
    assert!(password::verify(winning_password, &saved_user.password_hash).unwrap());
}

#[tokio::test]
async fn stale_expiry_update_cannot_overwrite_a_completed_request() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;
    let outcome = svc
        .request_password_reset(&actor, target_user_id, "expiry completion race", None)
        .await
        .expect("request password reset");

    let completion = db.begin().await.expect("begin completion transaction");
    let completed_at = outcome.request.expires_at - chrono::Duration::seconds(1);
    assert!(
        PasswordResetRequestRepository
            .complete_pending_in_txn(
                &completion,
                &actor.tenant_id,
                outcome.request.id,
                completed_at,
            )
            .await
            .expect("stage completion")
    );

    let expiry_db = db.connection().clone();
    let expiry_tenant = actor.tenant_id.clone();
    let request_id = outcome.request.id;
    let evaluated_at = outcome.request.expires_at + chrono::Duration::seconds(1);
    let mut expiry = tokio::spawn(async move {
        PasswordResetRequestRepository
            .expire_pending(&expiry_db, &expiry_tenant, request_id, evaluated_at)
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut expiry)
            .await
            .is_err(),
        "expiry update should wait for the completion row lock"
    );

    completion.commit().await.expect("commit completion");
    assert!(
        !expiry
            .await
            .expect("expiry task joins")
            .expect("expiry CAS succeeds")
    );
    let saved = PasswordResetRequestRepository
        .find_by_id(&db, &actor.tenant_id, request_id)
        .await
        .expect("find reset request")
        .expect("reset request exists");
    assert_eq!(
        saved.status,
        password_reset_request::Model::STATUS_COMPLETED
    );
    assert_eq!(saved.completed_at, Some(completed_at));
}

#[tokio::test]
async fn concurrent_super_role_promotion_blocks_password_reset() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;
    let outcome = svc
        .request_password_reset(&actor, target_user_id, "promotion race", None)
        .await
        .expect("request password reset");

    let now = chrono::Utc::now();
    let super_role = role::Model {
        id: 9_001,
        tenant_id: actor.tenant_id.clone(),
        name: "Super Role".into(),
        code: "test-super".into(),
        is_super: 1,
        data_scope: role::Model::DATA_SCOPE_ALL.into(),
        status: role::Model::STATUS_NORMAL.into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    };
    let active: role::ActiveModel = super_role.clone().into();
    active.insert(&db).await.expect("insert super role");

    let promotion = db.begin().await.expect("begin promotion transaction");
    RoleRepository
        .replace_roles_in_txn(
            &promotion,
            &actor.tenant_id,
            target_user_id,
            &[super_role.id],
        )
        .await
        .expect("stage super role promotion");

    let reset_service = user_service(&db);
    let tenant_id = actor.tenant_id.clone();
    let request_id = outcome.request.id;
    let token = outcome.token;
    let reset_task = tokio::spawn(async move {
        reset_service
            .complete_password_reset_request(&tenant_id, request_id, &token, "PromotedUser123!")
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    promotion
        .commit()
        .await
        .expect("commit super role promotion");

    let error = reset_task
        .await
        .expect("reset task should join")
        .expect_err("a promoted super user must not be reset");
    assert!(matches!(error, AppError::Authorization(_)));
    let saved_request = PasswordResetRequestRepository
        .find_by_id(&db, &actor.tenant_id, request_id)
        .await
        .expect("find reset request")
        .expect("reset request exists");
    assert_eq!(
        saved_request.status,
        password_reset_request::Model::STATUS_PENDING
    );
}

#[tokio::test]
async fn test_complete_password_reset_rejects_invalid_token() {
    let db = setup_test_db().await;
    let svc = user_service(&db);
    let actor = actor();
    let target_user_id = create_target_user(&svc, &actor).await;

    let outcome = svc
        .request_password_reset(&actor, target_user_id, "forgot password", None)
        .await
        .expect("request password reset");

    let err = svc
        .complete_password_reset_request(
            &actor.tenant_id,
            outcome.request.id,
            "wrong-token",
            "NewPass123!",
        )
        .await
        .expect_err("invalid token should be rejected");

    assert!(matches!(err, AppError::Authentication(_)));
}

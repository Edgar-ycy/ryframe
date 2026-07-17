mod common;

use common::setup_test_db;
use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, DataScope};
use ryframe_core::Repository;
use ryframe_db::{
    DatabaseCluster, PasswordResetRequestRepository, UserRepository,
    entities::{password_reset_request, user},
};
use ryframe_service::system::{CreateUserParams, UserService};

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

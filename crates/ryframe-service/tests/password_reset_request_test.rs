mod common;

use common::setup_test_db;
use ryframe_auth::password;
use ryframe_common::AppError;
use ryframe_core::{LoggedRepo, Repository};
use ryframe_db::{
    DeptRepository, PasswordResetRequestRepository, RoleRepository, UserRepository,
    entities::{password_reset_request, user},
};
use ryframe_service::system::{CreateUserParams, UserServiceImpl};

fn user_service() -> UserServiceImpl {
    UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
        redis: None,
    }
}

async fn create_target_user(svc: &UserServiceImpl, db: &sea_orm::DatabaseConnection) -> i64 {
    let user = svc
        .create(
            db,
            CreateUserParams {
                username: "reset_target",
                nickname: "Reset Target",
                email: "reset_target@test.com",
                phone: "13800000000",
                dept_id: None,
                role_ids: None,
            },
        )
        .await
        .expect("create target user");

    user.id.parse().expect("snowflake id should parse")
}

#[tokio::test]
async fn test_request_password_reset_persists_pending_request() {
    let db = setup_test_db().await;
    let svc = user_service();
    let target_user_id = create_target_user(&svc, &db).await;

    let outcome = svc
        .request_password_reset(
            &db,
            target_user_id,
            99,
            "  forgot password  ",
            Some("127.0.0.1".to_string()),
        )
        .await
        .expect("request password reset");
    let request = outcome.request;

    assert_eq!(request.target_user_id, target_user_id);
    assert_eq!(request.requested_by, 99);
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
        .find_pending_by_target(&db, target_user_id)
        .await
        .expect("find pending reset requests");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, request.id);
}

#[tokio::test]
async fn test_request_password_reset_requires_reason() {
    let db = setup_test_db().await;
    let svc = user_service();
    let target_user_id = create_target_user(&svc, &db).await;

    let err = svc
        .request_password_reset(&db, target_user_id, 99, "   ", None)
        .await
        .expect_err("blank reason should be rejected");

    assert!(matches!(err, AppError::Validation(_)));
}

#[tokio::test]
async fn test_complete_password_reset_sets_new_password_and_activates_pending_user() {
    let db = setup_test_db().await;
    let svc = user_service();
    let target_user_id = create_target_user(&svc, &db).await;

    let outcome = svc
        .request_password_reset(&db, target_user_id, 99, "activate account", None)
        .await
        .expect("request password reset");

    let completed_user_id = svc
        .complete_password_reset(&db, outcome.request.id, &outcome.token, "newpass123", false)
        .await
        .expect("complete password reset");

    assert_eq!(completed_user_id, target_user_id);

    let saved_user = UserRepository
        .find_by_id(&db, target_user_id)
        .await
        .expect("find user")
        .expect("user exists");
    assert_eq!(saved_user.status, user::Model::STATUS_NORMAL);
    assert!(password::verify("newpass123", &saved_user.password_hash).unwrap());

    let saved_request = PasswordResetRequestRepository
        .find_by_id(&db, outcome.request.id)
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
    let svc = user_service();
    let target_user_id = create_target_user(&svc, &db).await;

    let outcome = svc
        .request_password_reset(&db, target_user_id, 99, "forgot password", None)
        .await
        .expect("request password reset");

    let err = svc
        .complete_password_reset(&db, outcome.request.id, "wrong-token", "newpass123", false)
        .await
        .expect_err("invalid token should be rejected");

    assert!(matches!(err, AppError::Authentication(_)));
}

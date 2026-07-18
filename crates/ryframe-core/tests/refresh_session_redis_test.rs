use std::sync::Arc;

use ryframe_config::RedisConfig;
use ryframe_core::{RedisClient, RefreshFamily, RefreshRotation, RefreshSessionStore};

static REDIS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn family(sid: &str, current_jti: &str, absolute_exp: i64) -> RefreshFamily {
    RefreshFamily {
        sid: sid.into(),
        tenant_id: "system".into(),
        user_id: 1,
        current_jti: current_jti.into(),
        previous_jti: None,
        last_attempt_id: None,
        rotated_at: 0,
        absolute_exp,
        revoked: false,
    }
}

fn unique_sid(label: &str) -> String {
    format!(
        "sid-redis-{label}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros()
    )
}

async fn redis_store() -> (RedisClient, RefreshSessionStore) {
    let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16379);
    let redis = RedisClient::connect(&RedisConfig {
        port,
        database: 13,
        timeout_secs: 1,
        ..Default::default()
    })
    .await
    .expect("connect Docker Compose Redis service");
    let store = RefreshSessionStore::new(Some(redis.clone()));
    (redis, store)
}

/// Covers the production Lua CAS against the Compose Redis service, including
/// the exact grace-window boundaries and idempotent attempt recovery.
#[tokio::test]
#[ignore = "requires the Docker Compose Redis service on port 16379"]
async fn redis_refresh_rotation_cas_semantics() {
    let _guard = REDIS_TEST_LOCK.lock().await;
    let (_redis, store) = redis_store().await;
    let store = Arc::new(store);
    let now = chrono::Utc::now().timestamp();
    let sid = unique_sid("concurrent");
    store
        .register(family(&sid, "jti-initial", now + 600))
        .await
        .unwrap();

    let mut tasks = Vec::new();
    for index in 0..50 {
        let store = store.clone();
        let sid = sid.clone();
        tasks.push(tokio::spawn(async move {
            store
                .rotate(
                    &sid,
                    "jti-initial",
                    &format!("jti-winner-{index}"),
                    now,
                    &format!("attempt-{index}"),
                )
                .await
                .unwrap()
        }));
    }
    let mut rotated = 0;
    let mut concurrent = 0;
    for task in tasks {
        match task.await.unwrap() {
            RefreshRotation::Rotated { .. } => rotated += 1,
            RefreshRotation::Concurrent => concurrent += 1,
            unexpected => panic!("unexpected concurrent CAS result: {unexpected:?}"),
        }
    }
    assert_eq!(rotated, 1);
    assert_eq!(concurrent, 49);

    let recovery_sid = unique_sid("recovery");
    store
        .register(family(&recovery_sid, "jti-old", now + 600))
        .await
        .unwrap();
    assert_eq!(
        store
            .rotate(
                &recovery_sid,
                "jti-old",
                "jti-committed",
                now,
                "csrf-attempt",
            )
            .await
            .unwrap(),
        RefreshRotation::Rotated {
            current_jti: "jti-committed".into(),
            issued_at: now,
        }
    );
    assert_eq!(
        store
            .rotate(
                &recovery_sid,
                "jti-old",
                "jti-must-not-win",
                now + 30,
                "csrf-attempt",
            )
            .await
            .unwrap(),
        RefreshRotation::Recovered {
            current_jti: "jti-committed".into(),
            issued_at: now,
        }
    );

    let replay_sid = unique_sid("replay");
    store
        .register(family(&replay_sid, "jti-old", now + 600))
        .await
        .unwrap();
    assert!(matches!(
        store
            .rotate(
                &replay_sid,
                "jti-old",
                "jti-current",
                now,
                "attempt-original",
            )
            .await
            .unwrap(),
        RefreshRotation::Rotated { .. }
    ));
    assert_eq!(
        store
            .rotate(
                &replay_sid,
                "jti-old",
                "jti-other",
                now + 5,
                "attempt-concurrent",
            )
            .await
            .unwrap(),
        RefreshRotation::Concurrent
    );
    assert_eq!(
        store
            .rotate(
                &replay_sid,
                "jti-old",
                "jti-other",
                now + 6,
                "attempt-replay",
            )
            .await
            .unwrap(),
        RefreshRotation::Replayed
    );
    assert!(!store.is_active(&replay_sid).await.unwrap());

    let expiry_sid = unique_sid("expiry");
    let expiry_now = chrono::Utc::now().timestamp();
    store
        .register(family(&expiry_sid, "jti-old", expiry_now + 30))
        .await
        .unwrap();
    assert_eq!(
        store
            .rotate(
                &expiry_sid,
                "jti-old",
                "jti-new",
                expiry_now + 31,
                "attempt-expired",
            )
            .await
            .unwrap(),
        RefreshRotation::MissingOrRevoked
    );
    assert!(!store.is_active(&expiry_sid).await.unwrap());

    let first_device_sid = unique_sid("device-a");
    let second_device_sid = unique_sid("device-b");
    store
        .register(family(&first_device_sid, "jti-device-a", now + 600))
        .await
        .unwrap();
    store
        .register(family(&second_device_sid, "jti-device-b", now + 600))
        .await
        .unwrap();
    assert!(
        store
            .revoke_for_tenant("system", &first_device_sid)
            .await
            .unwrap()
    );
    assert!(!store.is_active(&first_device_sid).await.unwrap());
    assert!(store.is_active(&second_device_sid).await.unwrap());
}

/// Fault injection for the ambiguous-response path. The local tests assert
/// exact recovered metadata; this real Redis test ensures a timed-out CAS can
/// be retried with the same signed-attempt identity without revoking family.
#[tokio::test]
#[ignore = "requires the Docker Compose Redis service on port 16379"]
async fn redis_refresh_rotation_recovers_after_transient_response_loss() {
    let _guard = REDIS_TEST_LOCK.lock().await;
    let (redis, store) = redis_store().await;
    let now = chrono::Utc::now().timestamp();
    let sid = unique_sid("response-loss");
    store
        .register(family(&sid, "jti-old", now + 600))
        .await
        .unwrap();

    let mut connection = redis.conn().clone();
    redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(2_500)
        .arg("ALL")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let first = store
        .rotate(&sid, "jti-old", "jti-committed", now, "csrf-attempt-loss")
        .await;
    assert!(matches!(
        first,
        Err(ryframe_common::AppError::ServiceUnavailable(_))
    ));

    tokio::time::sleep(std::time::Duration::from_millis(1_750)).await;
    let retry = store
        .rotate(
            &sid,
            "jti-old",
            "jti-retry-proposal",
            now + 3,
            "csrf-attempt-loss",
        )
        .await
        .unwrap();
    // Depending on whether the connection manager cancelled before Redis
    // consumed the queued command, the retry either recovers the committed
    // first proposal or becomes the one CAS winner. Both are safe. A second
    // retry of the same attempt must converge on that exact committed value.
    let committed_jti = match retry {
        RefreshRotation::Rotated { current_jti, .. }
        | RefreshRotation::Recovered { current_jti, .. } => current_jti,
        unexpected => panic!("ambiguous CAS did not recover safely: {unexpected:?}"),
    };
    let converged = store
        .rotate(
            &sid,
            "jti-old",
            "jti-third-proposal",
            now + 4,
            "csrf-attempt-loss",
        )
        .await
        .unwrap();
    assert!(matches!(
        converged,
        RefreshRotation::Recovered { current_jti, .. } if current_jti == committed_jti
    ));
    assert!(store.is_active(&sid).await.unwrap());
}

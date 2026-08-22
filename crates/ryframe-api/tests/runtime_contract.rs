use axum::http::StatusCode;
use ryframe_api::{
    captcha::{challenge::*, *},
    metrics::*,
    middleware::idempotency::*,
    monitor::*,
};

mod captcha {
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    #[test]
    fn alphanumeric_challenge_uses_unambiguous_four_character_answer() {
        let mut rng = StdRng::seed_from_u64(7);
        let challenge = generate_challenge_with(&mut rng, CaptchaType::Alphanumeric);

        assert_eq!(challenge.display, challenge.answer);
        assert_eq!(challenge.answer.chars().count(), ALPHANUMERIC_LENGTH);
        assert!(
            challenge
                .answer
                .bytes()
                .all(|character| ALPHANUMERIC_ALPHABET.contains(&character))
        );
    }

    #[test]
    fn math_challenge_answer_matches_displayed_expression() {
        let mut rng = StdRng::seed_from_u64(19);
        let mut seen_operators = [false; 3];

        for _ in 0..32 {
            let challenge = generate_challenge_with(&mut rng, CaptchaType::Math);
            let expression = challenge
                .display
                .strip_suffix("=?")
                .expect("数学验证码应以等号和问号结尾");
            let (left, right, expected, operator_index) =
                if let Some((left, right)) = expression.split_once('+') {
                    let left = left.parse::<u32>().expect("加法左值应为数字");
                    let right = right.parse::<u32>().expect("加法右值应为数字");
                    (left, right, left + right, 0)
                } else if let Some((left, right)) = expression.split_once('-') {
                    let left = left.parse::<u32>().expect("减法左值应为数字");
                    let right = right.parse::<u32>().expect("减法右值应为数字");
                    assert!(left >= right, "减法验证码不应产生负数答案");
                    (left, right, left - right, 1)
                } else {
                    let (left, right) = expression
                        .split_once('×')
                        .expect("数学验证码应使用受支持的运算符");
                    let left = left.parse::<u32>().expect("乘法左值应为数字");
                    let right = right.parse::<u32>().expect("乘法右值应为数字");
                    (left, right, left * right, 2)
                };

            assert!((1..10).contains(&left));
            assert!((1..10).contains(&right));
            assert_eq!(
                challenge.answer.parse::<u32>().expect("答案应为数字"),
                expected
            );
            seen_operators[operator_index] = true;
        }

        assert!(seen_operators.into_iter().all(|seen| seen));
    }
}

mod readiness {
    use std::time::Duration;

    use super::{DependencyHealthCache, DependencyStatus};

    #[test]
    fn initial_snapshot_fails_closed() {
        let cache = DependencyHealthCache::new(true, true, Duration::MAX);
        let snapshot = cache.snapshot();

        assert!(!snapshot.is_ready());
        assert_eq!(snapshot.mysql, DependencyStatus::Unknown);
        assert_eq!(snapshot.redis, DependencyStatus::Unknown);
        assert_eq!(snapshot.object_storage, DependencyStatus::Unknown);
    }

    #[test]
    fn optional_dependencies_do_not_block_readiness() {
        let cache = DependencyHealthCache::new(false, false, Duration::MAX);
        cache.update(true, false, false);
        let snapshot = cache.snapshot();

        assert!(snapshot.is_ready());
        assert_eq!(snapshot.redis, DependencyStatus::OptionalDegraded);
        assert_eq!(snapshot.object_storage, DependencyStatus::NotRequired);
    }
}

mod metrics {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{ApiMetricsHooks, HttpRequestObservation};

    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static FINISHED: AtomicUsize = AtomicUsize::new(0);
    static ABANDONED: AtomicUsize = AtomicUsize::new(0);

    fn increment_started() {
        STARTED.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_finished(_: &str, _: &str, _: u16, _: std::time::Duration) {
        FINISHED.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_abandoned() {
        ABANDONED.fetch_add(1, Ordering::Relaxed);
    }

    fn noop() {}
    fn noop_str(_: &str) {}
    fn noop_static_str(_: &'static str) {}
    fn noop_usize(_: usize) {}
    fn noop_bool(_: bool) {}
    fn noop_duration(_: std::time::Duration) {}
    fn empty_text() -> String {
        String::new()
    }
    fn zero() -> u64 {
        0
    }
    fn empty_selections() -> super::DatabaseReadSelections {
        Vec::new()
    }

    #[test]
    fn observation_finishes_or_abandons_exactly_once() {
        super::install(ApiMetricsHooks {
            begin_http_request: increment_started,
            finish_http_request: increment_finished,
            abandon_http_request: increment_abandoned,
            metrics_text: empty_text,
            record_refresh_replay: noop,
            record_csrf_rejection: noop,
            record_redis_degraded: noop_str,
            record_idempotency_conflict: noop_str,
            record_rate_limit_rejection: noop_str,
            record_ws_ticket: noop_str,
            set_ws_connections: noop_usize,
            record_message_delivery: noop_str,
            set_message_redis_listener_connected: noop_bool,
            record_message_replay_query: noop_static_str,
            database_read_fallback_total: zero,
            database_read_selection_totals: empty_selections,
            observe_message_ack_latency: noop_duration,
        })
        .expect("测试指标钩子只能安装一次");

        HttpRequestObservation::start("GET".into(), "/readyz".into()).finish(200);
        drop(HttpRequestObservation::start(
            "POST".into(),
            "/cancelled".into(),
        ));

        assert_eq!(STARTED.load(Ordering::Relaxed), 2);
        assert_eq!(FINISHED.load(Ordering::Relaxed), 1);
        assert_eq!(ABANDONED.load(Ordering::Relaxed), 1);
    }
}

mod idempotency {
    use super::*;

    #[test]
    fn local_store_replays_only_the_same_fingerprint() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("测试运行时应创建成功")
            .block_on(async {
                let state = IdempotencyState::new(None, 300);
                assert!(matches!(
                    state.reserve("key", "fingerprint").await,
                    Ok(Reservation::Acquired)
                ));
                assert!(matches!(
                    state.reserve("key", "fingerprint").await,
                    Ok(Reservation::Processing)
                ));
                state
                    .complete(
                        "key",
                        "fingerprint",
                        CachedResponse {
                            status: StatusCode::CREATED.as_u16(),
                            body: b"created".to_vec(),
                            headers: Vec::new(),
                        },
                    )
                    .await
                    .expect("本地完成状态应写入成功");

                let Ok(Reservation::Completed(response)) =
                    state.reserve("key", "fingerprint").await
                else {
                    panic!("相同指纹应回放已完成响应");
                };
                assert_eq!(response.status, StatusCode::CREATED.as_u16());
                assert_eq!(response.body, b"created");
                assert!(matches!(
                    state.reserve("key", "different").await,
                    Ok(Reservation::Conflict)
                ));
            });
    }
}

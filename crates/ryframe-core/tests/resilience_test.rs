use std::{
    sync::{Arc, Barrier},
    thread,
};

use ryframe_core::resilience::{CircuitBreaker, CircuitState, RetryConfig, retry_with_backoff};

#[tokio::test]
async fn test_retry_with_backoff_success() {
    let mut counter = 0;
    let result = retry_with_backoff(
        || {
            counter += 1;
            async move {
                if counter < 3 {
                    Err("temporary error")
                } else {
                    Ok(42)
                }
            }
        },
        &RetryConfig::default(),
    )
    .await;

    assert_eq!(result, Ok(42));
    assert_eq!(counter, 3);
}

#[tokio::test]
async fn test_retry_with_backoff_exhausted() {
    let result = retry_with_backoff(
        || async { Err::<i32, _>("always fails") },
        &RetryConfig {
            max_retries: 2,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
            backoff_multiplier: 2.0,
        },
    )
    .await;

    assert!(result.is_err());
}

#[test]
fn test_circuit_breaker_basic() {
    let cb = CircuitBreaker::new(3, 60, 2);

    assert_eq!(cb.current_state(), CircuitState::Closed);
    assert!(cb.allow_request());

    cb.record_failure();
    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.current_state(), CircuitState::Open);
    assert!(!cb.allow_request());
}

#[test]
fn test_circuit_breaker_success_resets() {
    let cb = CircuitBreaker::new(3, 60, 2);

    cb.record_failure();
    cb.record_failure();
    cb.record_success();
    cb.record_failure();
    assert_eq!(cb.current_state(), CircuitState::Closed);
}

#[test]
fn half_open_limits_concurrent_probes() {
    const HALF_OPEN_MAX: usize = 3;
    const CONTENDERS: usize = 32;

    let cb = Arc::new(CircuitBreaker::new(1, 0, HALF_OPEN_MAX as u32));
    cb.record_failure();
    assert_eq!(cb.current_state(), CircuitState::Open);

    let barrier = Arc::new(Barrier::new(CONTENDERS));
    let handles = (0..CONTENDERS)
        .map(|_| {
            let cb = Arc::clone(&cb);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                cb.allow_request()
            })
        })
        .collect::<Vec<_>>();

    let allowed = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|allowed| *allowed)
        .count();

    assert_eq!(allowed, HALF_OPEN_MAX);
    assert_eq!(cb.current_state(), CircuitState::HalfOpen);
    assert!(!cb.allow_request());
}

#[test]
fn half_open_closes_only_after_all_permitted_probes_succeed() {
    let cb = CircuitBreaker::new(1, 0, 2);
    cb.record_failure();

    assert!(cb.allow_request());
    assert!(cb.allow_request());
    assert!(!cb.allow_request());

    cb.record_success();
    assert_eq!(cb.current_state(), CircuitState::HalfOpen);
    assert!(!cb.allow_request());

    cb.record_success();
    assert_eq!(cb.current_state(), CircuitState::Closed);
    assert!(cb.allow_request());
}

#[test]
#[should_panic(expected = "half_open_max must be greater than zero")]
fn half_open_requires_at_least_one_probe() {
    let _ = CircuitBreaker::new(1, 0, 0);
}

#[test]
fn any_half_open_failure_reopens_and_discards_outstanding_probes() {
    let cb = CircuitBreaker::new(1, 0, 2);
    cb.record_failure();
    assert!(cb.allow_request());
    assert!(cb.allow_request());

    cb.record_failure();
    assert_eq!(cb.current_state(), CircuitState::Open);

    // A completion from another formerly in-flight probe must not close the breaker.
    cb.record_success();
    assert_eq!(cb.current_state(), CircuitState::Open);
}

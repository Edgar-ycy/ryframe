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

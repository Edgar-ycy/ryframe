use super::*;
use serde_json::json;
use std::cell::Cell;

#[test]
fn transient_creation_and_image_errors_share_five_seconds_and_recover() {
    let clock = Cell::new(Duration::ZERO);
    let now = || clock.get();
    let pause = |duration| clock.set(clock.get() + duration);
    let mut creation_calls = 0;
    let creation = query_with(
        || {
            creation_calls += 1;
            if creation_calls < 3 {
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(1001_u64)
            }
        },
        || Ok(false),
        &now,
        pause,
        Duration::from_secs(5),
    )
    .unwrap();
    let mut image_calls = 0;
    let image = query_with(
        || {
            image_calls += 1;
            if image_calls == 1 {
                Err(io::Error::from_raw_os_error(31))
            } else {
                Ok("fixture.exe")
            }
        },
        || Ok(false),
        &now,
        pause,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(creation, Some(1001));
    assert_eq!(image, Some("fixture.exe"));
    assert_eq!((creation_calls, image_calls), (3, 2));
    assert_eq!(clock.get(), Duration::from_millis(150));
    // 创建时间已用去大部分预算时，映像查询不能另获完整五秒。
    clock.set(Duration::from_millis(4950));
    let error = query_with::<()>(
        || Err(io::Error::from_raw_os_error(31)),
        || Ok(false),
        &now,
        pause,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(31)
    );
    assert_eq!(clock.get(), Duration::from_secs(5));
}

#[test]
fn unix_first_esrch_preserves_false_while_sent_signal_and_failed_escalation_differ() {
    assert!(
        !super::super::stop_group(
            false,
            |_| Ok(false),
            || panic!("首次 ESRCH 直接进入外层 closure")
        )
        .unwrap()
    );
    let sent = Cell::new(0);
    assert!(
        super::super::stop_group(
            false,
            |kill| {
                assert!(!kill);
                sent.set(sent.get() + 1);
                Ok(true)
            },
            || Ok(true)
        )
        .unwrap()
    );
    assert_eq!(sent.get(), 1);
    let sent = Cell::new(0);
    assert!(
        super::super::stop_group(
            false,
            |kill| {
                sent.set(sent.get() + 1);
                assert_eq!(kill, sent.get() == 2);
                Ok(sent.get() == 1)
            },
            || Ok(false)
        )
        .is_err()
    );
    assert_eq!(sent.get(), 2);
    assert!(
        super::super::stop_group(
            true,
            |_| Err("拒绝发送信号".into()),
            || panic!("信号错误不能继续等待")
        )
        .is_err()
    );
}

#[test]
fn exit_during_query_error_allows_continuation_without_signal_and_reuse_rejects() {
    let expected = json!({"pid":101,"started":"1001","executable":"fixture.exe"});
    let observations = Cell::new(0);
    let signals = Cell::new(0);
    let stopped = control_with(
        &expected,
        || {
            query_with::<Value>(
                || Err(io::Error::from_raw_os_error(31)),
                || {
                    observations.set(observations.get() + 1);
                    Ok(observations.get() == 2)
                },
                &|| Duration::ZERO,
                |_| panic!("退出不应等待重试"),
                Duration::from_secs(5),
            )
        },
        || {
            signals.set(signals.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert!(!stopped);
    assert_eq!(signals.get(), 0);
    let mut reused = expected.clone();
    reused["started"] = json!("1002");
    assert!(
        control_with(
            &expected,
            || Ok(Some(reused)),
            || {
                signals.set(signals.get() + 1);
                Ok(())
            }
        )
        .is_err()
    );
    assert_eq!(signals.get(), 0);
    let stopped = control_with(
        &expected,
        || Ok(Some(expected.clone())),
        || {
            signals.set(signals.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert!(stopped);
    assert_eq!(signals.get(), 1);
}

#[test]
fn persistent_errors_expire_shared_deadline_and_other_errors_never_retry() {
    let clock = Cell::new(Duration::ZERO);
    let calls = Cell::new(0);
    let error = query_with::<()>(
        || {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(5))
        },
        || Ok(false),
        &|| clock.get(),
        |delay| clock.set(clock.get() + delay),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(5)
    );
    assert_eq!(calls.get(), 100);
    assert_eq!(clock.get(), Duration::from_secs(5));
    let error = query_with::<()>(
        || Err(io::Error::from_raw_os_error(87)),
        || Ok(false),
        &|| Duration::ZERO,
        |_| panic!("其他错误不得重试"),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(87)
    );
    assert!(
        query_with::<()>(
            || panic!("句柄已退出不得查询"),
            || Ok(true),
            &|| Duration::ZERO,
            |_| panic!("句柄已退出不得休眠"),
            Duration::from_secs(5)
        )
        .unwrap()
        .is_none()
    );
}

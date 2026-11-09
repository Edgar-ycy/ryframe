use super::*;

#[test]
fn stop_waits_for_sigterm_before_escalating_and_confirms_exit() {
    use std::cell::RefCell;

    let events = RefCell::new(Vec::new());
    for escalates in [false, true] {
        events.borrow_mut().clear();
        let stopped = stop_with(
            |signal| {
                events.borrow_mut().push((Some(signal), Duration::ZERO));
                Ok(true)
            },
            |timeout| {
                events.borrow_mut().push((None, timeout));
                Ok(!escalates || events.borrow().len() == 4)
            },
        )
        .unwrap();
        assert!(stopped);
        let mut expected = vec![
            (Some(Signal::TERM), Duration::ZERO),
            (None, Duration::from_secs(5)),
        ];
        if escalates {
            expected.extend([
                (Some(Signal::KILL), Duration::ZERO),
                (None, Duration::from_secs(5)),
            ]);
        }
        assert_eq!(*events.borrow(), expected);
    }
}

#[test]
fn stop_preserves_missing_process_and_unknown_exit_results() {
    assert!(
        !stop_with(
            |signal| {
                assert_eq!(signal, Signal::TERM);
                Ok(false)
            },
            |_| panic!("进程在首个信号前已退出，不再等待"),
        )
        .unwrap()
    );
    assert!(!stop_with(|signal| Ok(signal == Signal::TERM), |_| Ok(false),).unwrap());
    assert!(stop_with(|_| Ok(true), |_| Ok(false)).is_err());
    assert!(
        stop_with(
            |_| Err("发送失败".into()),
            |_| panic!("无法确认已发送信号时不得继续等待"),
        )
        .is_err()
    );
}

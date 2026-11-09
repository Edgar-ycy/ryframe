use super::*;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    future::{pending, ready},
    io,
};

fn run(future: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn retries_content_and_listener_then_rechecks_guard_before_success() {
    let history = RefCell::new(Vec::new());
    let round = Cell::new(0);
    let first_retry = Cell::new(None);
    run(attempts(
        Instant::now() + Duration::from_secs(5),
        || {
            history.borrow_mut().push("guard");
            Ok(())
        },
        || {
            history.borrow_mut().push("content");
            round.set(round.get() + 1);
            if round.get() == 1 {
                first_retry.set(Some(Instant::now()));
                ready(Err("未就绪".into()))
            } else {
                assert!(first_retry.get().unwrap().elapsed() >= RETRY);
                ready(Ok(()))
            }
        },
        || {
            history.borrow_mut().push("listener");
            Ok(round.get() != 2)
        },
    ))
    .unwrap();
    assert_eq!(
        *history.borrow(),
        [
            "guard", "content", "listener", "guard", "guard", "content", "listener", "guard",
            "guard", "content", "listener", "guard",
        ]
    );
}

#[test]
fn unknown_identity_or_failed_checkpoint_cannot_be_retried() {
    for fail_before in [true, false] {
        let probes = Cell::new(0);
        let guards = Cell::new(0);
        let result = run(attempts(
            Instant::now() + Duration::from_secs(5),
            || {
                guards.set(guards.get() + 1);
                if fail_before || guards.get() > 1 {
                    Err("未知内核身份".into())
                } else {
                    Ok(())
                }
            },
            || {
                probes.set(probes.get() + 1);
                ready(Err("HTTP 未就绪".into()))
            },
            || Ok(false),
        ));
        assert_eq!(result.unwrap_err().to_string(), "未知内核身份");
        assert_eq!(probes.get(), if fail_before { 0 } else { 1 });
        assert_eq!(guards.get(), if fail_before { 1 } else { 2 });
    }

    let guards = Cell::new(0);
    let result = run(attempts(
        Instant::now() + Duration::from_secs(5),
        || {
            guards.set(guards.get() + 1);
            if guards.get() == 1 {
                Ok(())
            } else {
                Err("父检查变化".into())
            }
        },
        || ready(Ok(())),
        || Ok(true),
    ));
    assert_eq!(result.unwrap_err().to_string(), "父检查变化");
}

#[test]
fn unknown_listener_owner_or_kernel_failure_aborts_even_if_http_is_not_ready() {
    for failure in ["监听被外来进程占用", "监听查询失败", "原创建身份未知"] {
        let rounds = Cell::new(0);
        let result = run(attempts(
            Instant::now() + Duration::from_secs(5),
            || Ok(()),
            || {
                rounds.set(rounds.get() + 1);
                ready(Err("HTTP 未就绪".into()))
            },
            || Err(failure.into()),
        ));
        assert_eq!(rounds.get(), 1);
        assert_eq!(result.unwrap_err().to_string(), failure);
    }
}

#[test]
fn deadline_bounds_inflight_work_and_prevents_further_attempts() {
    let probe = json!({"role":"worker","url":"http://127.0.0.1:12341/readyz"});
    let error = probes::within_deadline(&probe, Instant::now() + Duration::from_millis(80), |_| {
        pending::<Result<()>>()
    })
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::TimedOut
    );

    let error = run(attempts(
        Instant::now(),
        || panic!("已超时不能再检查父状态或查询身份"),
        || ready(Err("已超时不能再发探针".into())),
        || panic!("已超时不能再查询监听者"),
    ))
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn wrong_role_repeated_members_or_out_of_range_timeout_fail_before_observation() {
    let probe = json!({"role":"worker","url":"http://127.0.0.1:12341/readyz"});
    let mut tree = json!({"format_version":2,"kind":"full-stack-process-tree","role":"worker",
        "group_id":20,"supervisor":{"pid":20},"monitor":{"pid":21},"process":{"pid":22}});
    let checkpoint = || panic!("无效角色树或 timeout 不能触发任何外部检查");
    assert!(require_role(&tree, "worker").is_ok());
    assert!(wait(&tree, &probe, Duration::ZERO, checkpoint).is_err());
    assert!(wait(&tree, &probe, Duration::from_secs(181), checkpoint).is_err());
    tree["role"] = json!("api");
    assert!(wait(&tree, &probe, Duration::from_secs(1), checkpoint).is_err());
    tree["role"] = json!("worker");
    tree["process"]["pid"] = json!(20);
    assert!(wait(&tree, &probe, Duration::from_secs(1), checkpoint).is_err());
}

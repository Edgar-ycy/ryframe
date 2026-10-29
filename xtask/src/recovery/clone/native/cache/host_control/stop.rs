//! 关闭只使用本次整次原语的固定预算；停止请求最多发送一次。
use super::super::diagnostic::stop::{Stage, capture};
use super::*;

pub(super) fn run(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
    mut guard: impl FnMut() -> Result<()>,
    wait_port: bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut phase = Stage::Inputs;
    let mut sent = false;
    let mut exited = false;
    let outcome = (|| {
        model::validate(backend, request, true)?;
        common::local_directory(backend, output, "缓存停止输出")?;
        let tool = native_tool::from_request(request)?;
        tool.finish(backend)?;
        guard()?;
        expired(deadline)?;
        if runtime.get("partial_receipt").is_some() {
            let status = super::super::readonly::status(backend, request, runtime, None)?;
            if status["state"] != "stopped" {
                return Err("缓存部分启动未证明关闭".into());
            }
            let result = json!({"state":"stopped","status":"redis_process_stopped","runtime":runtime,"resources_deleted":false});
            phase = Stage::PartialPublish;
            expired(deadline)?;
            super::super::linux_write::publish(&output.join("stopped.json"), &result)?;
            phase = Stage::Guard;
            expired(deadline)?;
            guard()?;
            tool.finish(backend)?;
            expired(deadline)?;
            return Ok(result);
        }
        launcher(request, runtime, false)?;
        expired(deadline)?;
        phase = Stage::Stop;
        sent = true;
        let (mut result, recovered) =
            stop_once(|action| process::call(backend, request, runtime, action))?;
        phase = Stage::Exit;
        if result["alive"] != false {
            return Err("缓存停止后仍在运行".into());
        }
        exited = true;
        complete(
            backend, request, runtime, deadline, &mut guard, &mut phase, wait_port,
        )?;
        result["status"] = json!("redis_process_stopped");
        result["runtime"] = runtime.clone();
        result["resources_deleted"] = json!(false);
        if recovered {
            result["observed_after_failed_stop"] = json!(true);
        }
        phase = Stage::Publish;
        super::super::linux_write::publish(&output.join("stopped.json"), &result)?;
        phase = Stage::Guard;
        guard()?;
        expired(deadline)?;
        Ok(result)
    })();
    outcome.map_err(|error| capture(phase, sent, exited, error))
}

fn complete(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    deadline: Instant,
    guard: &mut impl FnMut() -> Result<()>,
    phase: &mut Stage,
    wait_port: bool,
) -> Result<()> {
    let tool = native_tool::from_request(request)?;
    *phase = Stage::Launcher;
    let launcher_deadline = (Instant::now() + Duration::from_secs(10)).min(deadline);
    while launcher(request, runtime, false)? {
        expired(launcher_deadline)?;
        guard()?;
        tool.finish(backend)?;
        std::thread::sleep(
            launcher_deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
    expired(launcher_deadline)?;
    *phase = Stage::Port;
    if !wait_port {
        bind_and_close(request)?;
    } else {
        let released = wait(
            deadline,
            &mut || {
                guard()?;
                tool.finish(backend)?;
                model::validate(backend, request, true)?;
                if launcher(request, runtime, false)? {
                    return Err("Redis launcher重新出现".into());
                }
                let observed = evidence::inspect(
                    request,
                    Path::new(doc::text(&runtime["output"], "原缓存目录")?),
                )?;
                if observed.as_object().is_none_or(|fields| {
                    fields
                        .iter()
                        .any(|(key, value)| runtime.get(key) != Some(value))
                }) {
                    return Err("原缓存创建身份收据变化".into());
                }
                Ok(())
            },
            || bind_after_stop(request, true),
            Instant::now,
            std::thread::sleep,
        );
        if Instant::now() >= deadline
            && released
                .as_ref()
                .err()
                .is_some_and(|error| error.downcast_ref::<std::io::Error>().is_some())
        {
            *phase = Stage::PortReleaseTimeout;
        }
        released?;
    }
    *phase = Stage::Guard;
    tool.finish(backend)?;
    model::validate(backend, request, true)?;
    if launcher(request, runtime, false)? {
        return Err("关闭端口核验后launcher身份变化".into());
    }
    guard()?;
    expired(deadline)?;
    Ok(())
}

fn expired(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
    }
    Ok(())
}

pub(super) fn wait(
    deadline: Instant,
    guard: &mut impl FnMut() -> Result<()>,
    mut bind: impl FnMut() -> Result<()>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<()> {
    let mut last = None;
    loop {
        if now() >= deadline {
            break;
        }
        guard()?;
        if now() >= deadline {
            break;
        }
        match bind() {
            Ok(()) => {
                if now() >= deadline {
                    break;
                }
                guard()?;
                if now() < deadline {
                    return Ok(());
                }
                break;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse) =>
            {
                last = Some(error)
            }
            Err(error) => return Err(error),
        }
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() {
            break;
        }
        sleep(remaining.min(Duration::from_millis(500)));
    }
    Err(last.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut).into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn after_stop_release_requires_real_bind_with_both_guards_and_original_deadline() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let request = json!({"port":listener.local_addr().unwrap().port()});
        let mut listener = Some(listener);
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let guards = Cell::new(0);
        wait(
            start + Duration::from_millis(750),
            &mut || {
                guards.set(guards.get() + 1);
                Ok(())
            },
            || bind_and_close(&request),
            || start + elapsed.get(),
            |duration| {
                elapsed.set(elapsed.get() + duration);
                drop(listener.take());
            },
        )
        .unwrap();
        assert_eq!(elapsed.get(), Duration::from_millis(500));
        assert_eq!(guards.get(), 3);
        for late in [false, true] {
            elapsed.set(Duration::ZERO);
            let calls = Cell::new(0);
            let error = wait(
                start + Duration::from_millis(750),
                &mut || Ok(()),
                || {
                    calls.set(calls.get() + 1);
                    if late && calls.get() == 2 {
                        elapsed.set(Duration::from_millis(750));
                        Ok(())
                    } else {
                        Err(std::io::Error::from_raw_os_error(if cfg!(windows) {
                            10048
                        } else {
                            98
                        })
                        .into())
                    }
                },
                || start + elapsed.get(),
                |duration| {
                    assert!(duration <= Duration::from_millis(500));
                    elapsed.set(elapsed.get() + duration);
                },
            )
            .unwrap_err();
            assert_eq!(elapsed.get(), Duration::from_millis(750));
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(if cfg!(windows) { 10048 } else { 98 })
            );
        }
    }

    #[test]
    fn after_stop_guard_and_non_occupied_error_fail_without_sleep_and_diagnostics_never_echo_cause()
    {
        let start = Instant::now();
        let error = wait(
            start + Duration::from_secs(1),
            &mut || Err("private-token".into()),
            || panic!("守卫失败不能绑定"),
            || start,
            |_| panic!("不能等待"),
        )
        .unwrap_err();
        let failure = capture(Stage::Guard, true, true, error);
        let (detail, code) = diagnostic::primitive_detail(failure.as_ref());
        assert!(!detail.to_string().contains("private-token"));
        assert_eq!(detail["stop_requested"], true);
        assert_eq!(detail["exit_confirmed"], true);
        let decoded = diagnostic::primitive_error(&detail, &code).unwrap();
        assert_eq!(
            diagnostic::primitive_detail(decoded.as_ref()),
            (detail.clone(), code)
        );
        let remote = diagnostic::primitive_error(&Value::Null, &json!(17)).unwrap();
        let remote = capture(Stage::Stop, true, false, remote);
        let (remote_detail, remote_code) = diagnostic::primitive_detail(remote.as_ref());
        assert_eq!(remote_code, 17);
        assert_eq!(
            diagnostic::primitive_detail(
                diagnostic::primitive_error(&remote_detail, &remote_code)
                    .unwrap()
                    .as_ref()
            ),
            (remote_detail, remote_code)
        );
        let cleanup = super::super::super::cleanup_diagnostic::capture(remote.as_ref());
        super::super::super::cleanup_diagnostic::validate(&cleanup).unwrap();
        let partial = capture(Stage::PartialPublish, false, false, "private-token".into());
        let (partial_detail, partial_code) = diagnostic::primitive_detail(partial.as_ref());
        assert_eq!(partial_detail["stage"], "partial_publish");
        assert_eq!(partial_detail["stop_requested"], false);
        assert_eq!(partial_detail["exit_confirmed"], false);
        assert!(!partial_detail.to_string().contains("private-token"));
        let decoded = diagnostic::primitive_error(&partial_detail, &partial_code).unwrap();
        assert_eq!(
            diagnostic::primitive_detail(decoded.as_ref()),
            (partial_detail.clone(), partial_code)
        );
        for field in ["stop_requested", "exit_confirmed"] {
            let mut bad = partial_detail.clone();
            bad[field] = json!(true);
            assert!(diagnostic::primitive_error(&bad, &Value::Null).is_err());
        }
        let mut normal_publish = partial_detail.clone();
        normal_publish["stage"] = json!("publish");
        assert!(diagnostic::primitive_error(&normal_publish, &Value::Null).is_err());
        for (key, value) in [
            ("cause", json!("private-token")),
            ("extra", json!("private-token")),
            ("os_code", json!("private-token")),
        ] {
            let mut bad = detail.clone();
            bad[key] = value;
            assert!(diagnostic::primitive_error(&bad, &Value::Null).is_err());
        }
        let error = wait(
            start + Duration::from_secs(1),
            &mut || Ok(()),
            || Err(std::io::Error::from_raw_os_error(5).into()),
            || start,
            |_| panic!("非占用错误不得等待"),
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
    }
}

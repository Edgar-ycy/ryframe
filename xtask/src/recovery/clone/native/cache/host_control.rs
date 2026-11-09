//! 本机 Windows Redis 的观察、停止与独占端口检查。
use super::diagnostic;
use crate::Result;
#[cfg(any(test, windows))]
use diagnostic::Category;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::path::Path;
#[cfg(any(test, windows))]
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::{Duration, Instant},
};

#[cfg(any(test, windows))]
pub(super) fn bind_and_close(request: &Value) -> Result<()> {
    let port = u16::try_from(request["port"].as_u64().ok_or("缓存端口无效")?)?;
    bind_once(port).map_err(|(_, error)| error.into())
}
#[cfg(any(test, windows))]
fn bind_once(port: u16) -> std::result::Result<(), (Category, std::io::Error)> {
    let socket = tokio::net::TcpSocket::new_v4()
        .map_err(|error| (Category::PrelaunchSocketFailed, error))?;
    socket
        .set_reuseaddr(false)
        .map_err(|error| (Category::PrelaunchSocketOptionsFailed, error))?;
    socket
        .bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())
        .map_err(|error| (Category::PrelaunchPortBindFailed, error))?;
    drop(socket);
    Ok(())
}

#[cfg(any(test, windows))]
pub(super) fn wait_before_start(
    request: &Value,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    wait_for_release(
        deadline,
        guard,
        || {
            let port = u16::try_from(request["port"].as_u64().ok_or_else(|| {
                (
                    Category::PrelaunchPortBindFailed,
                    std::io::Error::from(std::io::ErrorKind::InvalidInput),
                )
            })?)
            .map_err(|_| {
                (
                    Category::PrelaunchPortBindFailed,
                    std::io::Error::from(std::io::ErrorKind::InvalidInput),
                )
            })?;
            bind_once(port)
        },
        Instant::now,
        std::thread::sleep,
    )
}

#[cfg(any(test, windows))]
fn wait_for_release(
    deadline: Instant,
    guard: &mut impl FnMut() -> Result<()>,
    mut bind: impl FnMut() -> std::result::Result<(), (Category, std::io::Error)>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<()> {
    let mut last = None;
    loop {
        if now() >= deadline {
            break;
        }
        guard().map_err(|error| diagnostic::prelaunch(Category::PrelaunchGuardRejected, error))?;
        if now() >= deadline {
            break;
        }
        match bind() {
            Ok(()) if now() < deadline => return Ok(()),
            Ok(()) => break,
            Err((Category::PrelaunchPortBindFailed, error))
                if error.kind() == std::io::ErrorKind::AddrInUse =>
            {
                last = Some(error)
            }
            Err((category, error)) => return Err(diagnostic::prelaunch(category, error.into())),
        }
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() {
            break;
        }
        sleep(remaining.min(Duration::from_millis(500)));
    }
    Err(diagnostic::prelaunch(
        Category::PrelaunchPortReleaseTimeout,
        last.unwrap_or_else(|| std::io::ErrorKind::TimedOut.into())
            .into(),
    ))
}
#[cfg(test)]
#[tokio::test]
async fn occupied_prelaunch_port_keeps_bind_stage_and_os_code_across_diagnostic_wire() {
    use super::diagnostic::{primitive_detail, primitive_error};
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let request = json!({"port":listener.local_addr().unwrap().port()});
    let error = bind_once(u16::try_from(request["port"].as_u64().unwrap()).unwrap())
        .map_err(|(category, error)| diagnostic::prelaunch(category, error.into()))
        .unwrap_err();
    let (detail, code) = primitive_detail(error.as_ref());
    assert_eq!(detail["category"], "prelaunch_port_bind_failed");
    assert_eq!(detail["cause"], "address_in_use");
    assert!(detail["os_code"].is_i64());
    assert_eq!(detail["write_outcome"], "not_started");
    let decoded = primitive_error(&detail, &code).unwrap();
    assert_eq!(primitive_detail(decoded.as_ref()), (detail.clone(), code));
    for (key, value) in [
        ("cause", json!("environment_changed")),
        ("write_outcome", json!("unknown")),
    ] {
        let mut changed = detail.clone();
        changed[key] = value;
        assert!(primitive_error(&changed, &Value::Null).is_err());
    }
    let stopped = bind_and_close(&request).unwrap_err();
    let io = stopped.downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::AddrInUse);
    assert_eq!(json!(io.raw_os_error()), detail["os_code"]);
    assert_eq!(
        primitive_detail(stopped.as_ref()),
        (Value::Null, Value::Null)
    );
    assert!(super::diagnostic::primitive_message(stopped.as_ref()).is_none());
    drop(listener);
    bind_once(u16::try_from(request["port"].as_u64().unwrap()).unwrap()).unwrap();
    bind_and_close(&request).unwrap();
}

pub(crate) fn observe(backend: &Path, request: &Value, runtime: &Value) -> Result<Value> {
    super::require_windows(request)?;
    #[cfg(windows)]
    return super::windows::observe(backend, request, runtime);
    #[cfg(not(windows))]
    {
        let _ = (backend, runtime);
        Err("Windows Redis 原生观察只支持本机 Windows".into())
    }
}

pub(crate) fn stop(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
) -> Result<Value> {
    stop_with(backend, request, runtime, output, || Ok(()), false)
}

pub(crate) fn stop_guarded(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
    guard: impl FnMut() -> Result<()>,
) -> Result<Value> {
    stop_with(backend, request, runtime, output, guard, true)
}

fn stop_with(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
    guard: impl FnMut() -> Result<()>,
    wait: bool,
) -> Result<Value> {
    super::require_windows(request)?;
    #[cfg(windows)]
    return super::windows::stop(backend, request, runtime, output, guard, wait);
    #[cfg(not(windows))]
    {
        let _ = (backend, runtime, output, guard, wait);
        Err("Windows Redis 原生停止只支持本机 Windows".into())
    }
}

pub(crate) fn closed_port(backend: &Path, request: &Value, runtime: &Value) -> Result<()> {
    let observation = super::readonly::status(backend, request, runtime, None)?;
    if observation["state"] != "stopped" {
        return Err("原 Windows Redis 代次尚未关闭".into());
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn bind_after_stop(request: &Value, requested: bool) -> Result<()> {
    let port = u16::try_from(request["port"].as_u64().ok_or("缓存端口无效")?)?;
    bind_once(port).map_err(|(category, error)| {
        if matches!(category, Category::PrelaunchPortBindFailed)
            && error.kind() == std::io::ErrorKind::AddrInUse
        {
            error.into()
        } else {
            diagnostic::stop::capture(diagnostic::stop::Stage::Port, requested, true, error.into())
        }
    })
}

pub(crate) fn recovery_port_error(error: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
    diagnostic::stop::capture(diagnostic::stop::Stage::Port, false, true, error)
}
#[cfg(test)]
#[tokio::test]
async fn prelaunch_wait_allows_only_actual_port_release_within_original_budget() {
    use std::cell::Cell;
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut listener = Some(listener);
    let start = Instant::now();
    let elapsed = Cell::new(Duration::ZERO);
    let guards = Cell::new(0);
    wait_for_release(
        start + Duration::from_secs(60),
        &mut || {
            guards.set(guards.get() + 1);
            Ok(())
        },
        || bind_once(port),
        || start + elapsed.get(),
        |duration| {
            elapsed.set(elapsed.get() + duration);
            drop(listener.take());
        },
    )
    .unwrap();
    assert_eq!(guards.get(), 2);
    assert_eq!(elapsed.get(), Duration::from_millis(500));
}

#[cfg(test)]
#[test]
fn prelaunch_wait_timeout_keeps_last_os_code_and_rejects_late_success() {
    use super::diagnostic::{primitive_detail, primitive_error};
    use std::cell::Cell;
    for (late_success, slow_guard) in [(false, false), (true, false), (false, true)] {
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let attempts = Cell::new(0);
        let os_code = if cfg!(windows) { 10048 } else { 98 };
        let error = wait_for_release(
            start + Duration::from_millis(750),
            &mut || {
                if slow_guard && attempts.get() > 0 {
                    elapsed.set(Duration::from_millis(750));
                }
                Ok(())
            },
            || {
                attempts.set(attempts.get() + 1);
                if late_success && attempts.get() == 2 {
                    elapsed.set(Duration::from_millis(750));
                    Ok(())
                } else {
                    Err((
                        Category::PrelaunchPortBindFailed,
                        std::io::Error::from_raw_os_error(os_code),
                    ))
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
        assert_eq!(attempts.get(), if slow_guard { 1 } else { 2 });
        let (detail, code) = primitive_detail(error.as_ref());
        assert_eq!(detail["category"], "prelaunch_port_release_timeout");
        assert_eq!(detail["os_code"], os_code);
        assert_eq!(detail["write_outcome"], "not_started");
        assert_eq!(
            primitive_detail(primitive_error(&detail, &code).unwrap().as_ref()),
            (detail.clone(), code.clone())
        );
        for (key, value) in [
            ("cause", json!("other_io")),
            ("write_outcome", json!("unknown")),
            ("category", json!("unrecognized")),
        ] {
            let mut changed = detail.clone();
            changed[key] = value;
            assert!(primitive_error(&changed, &code).is_err());
        }
    }
}

#[cfg(test)]
#[test]
fn prelaunch_wait_guard_drift_and_non_bind_or_non_occupied_errors_fail_immediately() {
    use super::diagnostic::primitive_detail;
    use std::cell::Cell;
    for (category, kind) in [
        (
            Category::PrelaunchSocketFailed,
            std::io::ErrorKind::AddrInUse,
        ),
        (
            Category::PrelaunchSocketOptionsFailed,
            std::io::ErrorKind::AddrInUse,
        ),
        (
            Category::PrelaunchPortBindFailed,
            std::io::ErrorKind::PermissionDenied,
        ),
    ] {
        let start = Instant::now();
        let calls = Cell::new(0);
        let error = wait_for_release(
            start + Duration::from_secs(60),
            &mut || Ok(()),
            || {
                calls.set(calls.get() + 1);
                Err((category, kind.into()))
            },
            || start,
            |_| panic!("非占用错误不得等待"),
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(
            primitive_detail(error.as_ref()).0["cause"],
            if kind == std::io::ErrorKind::AddrInUse {
                "address_in_use"
            } else {
                "permission_denied"
            }
        );
    }
    let start = Instant::now();
    let elapsed = Cell::new(Duration::ZERO);
    let calls = Cell::new(0);
    let error = wait_for_release(
        start + Duration::from_secs(60),
        &mut || {
            if calls.get() > 0 {
                Err("生命周期私有环境在执行期间变化".into())
            } else {
                Ok(())
            }
        },
        || {
            calls.set(calls.get() + 1);
            Err((
                Category::PrelaunchPortBindFailed,
                std::io::ErrorKind::AddrInUse.into(),
            ))
        },
        || start + elapsed.get(),
        |duration| elapsed.set(elapsed.get() + duration),
    )
    .unwrap_err();
    assert_eq!(calls.get(), 1);
    assert_eq!(
        primitive_detail(error.as_ref()).0["cause"],
        "environment_changed"
    );
}

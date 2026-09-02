use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use crate::{Result, process::ManagedChild};

use super::{
    model::{CycleControl, HEALTH_TIMEOUT, LOOP_INTERVAL},
    services::ensure_running,
};

const READYZ_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const READYZ_IO_TIMEOUT: Duration = Duration::from_millis(200);
const PROBE_CONTROL_SLICE: Duration = Duration::from_millis(25);

pub(crate) fn combine_failures<T, const N: usize>(
    result: Result<T>,
    stops: [(&str, Result<()>); N],
) -> Result<T> {
    let mut errors = Vec::new();
    let value = match result {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    for (label, stop) in stops {
        if let Err(error) = stop {
            errors.push(format!("{label} 失败：{error}"));
        }
    }
    if errors.is_empty() {
        Ok(value.expect("没有错误时必须保留主操作结果"))
    } else {
        Err(errors.join("；").into())
    }
}

pub(crate) fn start_worker_after_api_ready<T>(
    wait_for_api: impl FnOnce() -> Result<()>,
    start_worker: impl FnOnce() -> Result<T>,
) -> Result<T> {
    wait_for_api()?;
    start_worker()
}

pub(super) fn wait_healthy_until(
    child: &mut ManagedChild,
    port: u16,
    label: &str,
    health_deadline: Instant,
) -> Result<()> {
    loop {
        ensure_running(label, child)?;
        if Instant::now() >= health_deadline {
            return Err(format!(
                "{label} 未在共享 {} 秒截止时间内通过 /readyz",
                HEALTH_TIMEOUT.as_secs()
            )
            .into());
        }
        let ready = http_readyz(port);
        if ready && Instant::now() <= health_deadline {
            ensure_running(label, child)?;
            return Ok(());
        }
        thread::sleep(LOOP_INTERVAL.min(health_deadline.saturating_duration_since(Instant::now())));
    }
}

pub(crate) fn wait_services_ready_until<EA, EW, RA, RW>(
    health_deadline: Instant,
    api_label: &str,
    worker_label: &str,
    ensure_api_running: EA,
    ensure_worker_running: EW,
    api_ready: RA,
    worker_ready: RW,
) -> Result<()>
where
    EA: FnMut() -> Result<()>,
    EW: FnMut() -> Result<()>,
    RA: FnMut() -> bool,
    RW: FnMut() -> bool,
{
    match wait_services_ready_until_controlled(
        health_deadline,
        api_label,
        worker_label,
        ensure_api_running,
        ensure_worker_running,
        api_ready,
        worker_ready,
        || CycleControl::Continue,
    )? {
        CycleControl::Continue => Ok(()),
        CycleControl::Superseded | CycleControl::Shutdown => {
            Err("无控制源的健康检查不应被取消".into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wait_services_ready_until_controlled<EA, EW, RA, RW, C>(
    health_deadline: Instant,
    api_label: &str,
    worker_label: &str,
    mut ensure_api_running: EA,
    mut ensure_worker_running: EW,
    mut api_ready: RA,
    mut worker_ready: RW,
    mut control: C,
) -> Result<CycleControl>
where
    EA: FnMut() -> Result<()>,
    EW: FnMut() -> Result<()>,
    RA: FnMut() -> bool,
    RW: FnMut() -> bool,
    C: FnMut() -> CycleControl,
{
    loop {
        let current = control();
        if current != CycleControl::Continue {
            return Ok(current);
        }
        ensure_api_running()?;
        ensure_worker_running()?;
        if Instant::now() >= health_deadline {
            return Err(format!(
                "{api_label} 与 {worker_label} 未在共享 {} 秒截止时间内同时通过 /readyz",
                HEALTH_TIMEOUT.as_secs()
            )
            .into());
        }
        let api_is_ready = api_ready();
        let current = control();
        if current != CycleControl::Continue {
            return Ok(current);
        }
        if Instant::now() >= health_deadline {
            return Err(format!(
                "{api_label} 与 {worker_label} 未在共享 {} 秒截止时间内同时通过 /readyz",
                HEALTH_TIMEOUT.as_secs()
            )
            .into());
        }
        let worker_is_ready = worker_ready();
        let current = control();
        if current != CycleControl::Continue {
            return Ok(current);
        }
        if api_is_ready && worker_is_ready && Instant::now() <= health_deadline {
            ensure_api_running()?;
            ensure_worker_running()?;
            return Ok(CycleControl::Continue);
        }
        thread::sleep(LOOP_INTERVAL.min(health_deadline.saturating_duration_since(Instant::now())));
    }
}

/// 候选 probe 使用短 I/O 时间片，并在每个潜在阻塞点后重新读取源码代次。
/// 该循环不派生后台线程，因此 supersede 或 shutdown 返回时没有探活工作遗留。
#[allow(clippy::too_many_arguments)]
pub(crate) fn wait_probe_services_ready_until_controlled<EA, EW, C>(
    health_deadline: Instant,
    api_label: &str,
    worker_label: &str,
    mut ensure_api_running: EA,
    mut ensure_worker_running: EW,
    api_port: u16,
    worker_port: u16,
    mut control: C,
) -> Result<CycleControl>
where
    EA: FnMut() -> Result<()>,
    EW: FnMut() -> Result<()>,
    C: FnMut() -> CycleControl,
{
    loop {
        if let Some(current) = interrupted(&mut control) {
            return Ok(current);
        }
        ensure_api_running()?;
        ensure_worker_running()?;
        ensure_before_deadline(health_deadline, api_label, worker_label)?;

        let api_is_ready = match controlled_http_readyz(api_port, health_deadline, &mut control)? {
            ControlledReady::Ready(ready) => ready,
            ControlledReady::Interrupted(current) => return Ok(current),
        };
        ensure_before_deadline(health_deadline, api_label, worker_label)?;
        let worker_is_ready =
            match controlled_http_readyz(worker_port, health_deadline, &mut control)? {
                ControlledReady::Ready(ready) => ready,
                ControlledReady::Interrupted(current) => return Ok(current),
            };
        if api_is_ready && worker_is_ready {
            ensure_api_running()?;
            ensure_worker_running()?;
            return Ok(CycleControl::Continue);
        }
        thread::sleep(
            PROBE_CONTROL_SLICE.min(health_deadline.saturating_duration_since(Instant::now())),
        );
    }
}

enum ControlledReady {
    Ready(bool),
    Interrupted(CycleControl),
}

fn controlled_http_readyz<C>(
    port: u16,
    deadline: Instant,
    control: &mut C,
) -> Result<ControlledReady>
where
    C: FnMut() -> CycleControl,
{
    if let Some(current) = interrupted(control) {
        return Ok(ControlledReady::Interrupted(current));
    }
    let timeout = io_slice(deadline);
    if timeout.is_zero() {
        return Ok(ControlledReady::Ready(false));
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, timeout) {
        Ok(stream) => stream,
        Err(_) => return after_io(false, control),
    };
    if let Some(current) = interrupted(control) {
        return Ok(ControlledReady::Interrupted(current));
    }
    let timeout = io_slice(deadline);
    if timeout.is_zero() {
        return Ok(ControlledReady::Ready(false));
    }
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    if stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return after_io(false, control);
    }
    if let Some(current) = interrupted(control) {
        return Ok(ControlledReady::Interrupted(current));
    }
    let timeout = io_slice(deadline);
    if timeout.is_zero() {
        return Ok(ControlledReady::Ready(false));
    }
    stream.set_read_timeout(Some(timeout))?;
    let mut response = [0_u8; 64];
    let ready = stream.read(&mut response).is_ok_and(|length| {
        response[..length].starts_with(b"HTTP/1.1 200")
            || response[..length].starts_with(b"HTTP/1.0 200")
    });
    after_io(ready, control)
}

fn after_io<C>(ready: bool, control: &mut C) -> Result<ControlledReady>
where
    C: FnMut() -> CycleControl,
{
    Ok(match interrupted(control) {
        Some(current) => ControlledReady::Interrupted(current),
        None => ControlledReady::Ready(ready),
    })
}

fn interrupted<C>(control: &mut C) -> Option<CycleControl>
where
    C: FnMut() -> CycleControl,
{
    let current = control();
    (current != CycleControl::Continue).then_some(current)
}

fn io_slice(deadline: Instant) -> Duration {
    PROBE_CONTROL_SLICE.min(deadline.saturating_duration_since(Instant::now()))
}

fn ensure_before_deadline(deadline: Instant, api_label: &str, worker_label: &str) -> Result<()> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(format!(
            "{api_label} 与 {worker_label} 未在共享 {} 秒截止时间内同时通过 /readyz",
            HEALTH_TIMEOUT.as_secs()
        )
        .into())
    }
}

pub(super) fn http_readyz(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, READYZ_CONNECT_TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(READYZ_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READYZ_IO_TIMEOUT));
    if stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = [0_u8; 64];
    let Ok(length) = stream.read(&mut response) else {
        return false;
    };
    response[..length].starts_with(b"HTTP/1.1 200")
        || response[..length].starts_with(b"HTTP/1.0 200")
}

pub(crate) fn available_ports() -> Result<(u16, u16)> {
    let api = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let worker = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok((api.local_addr()?.port(), worker.local_addr()?.port()))
}

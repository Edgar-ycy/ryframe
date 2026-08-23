use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    process::Child,
    thread,
    time::{Duration, Instant},
};

use crate::Result;

use super::{
    model::{HEALTH_TIMEOUT, LOOP_INTERVAL},
    services::ensure_running,
};

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
    child: &mut Child,
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
    mut ensure_api_running: EA,
    mut ensure_worker_running: EW,
    mut api_ready: RA,
    mut worker_ready: RW,
) -> Result<()>
where
    EA: FnMut() -> Result<()>,
    EW: FnMut() -> Result<()>,
    RA: FnMut() -> bool,
    RW: FnMut() -> bool,
{
    loop {
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
        let worker_is_ready = worker_ready();
        if api_is_ready && worker_is_ready && Instant::now() <= health_deadline {
            ensure_api_running()?;
            ensure_worker_running()?;
            return Ok(());
        }
        thread::sleep(LOOP_INTERVAL.min(health_deadline.saturating_duration_since(Instant::now())));
    }
}

pub(super) fn http_readyz(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
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

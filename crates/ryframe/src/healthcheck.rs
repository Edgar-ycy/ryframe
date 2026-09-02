use std::{
    env,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    time::Duration,
};

use ryframe_kernel::AppError;

const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// 从环境变量读取本进程健康端口，并探测本机 `/readyz`。
pub fn probe_from_env(variable: &str, default_port: u16) -> Result<(), AppError> {
    let port = match env::var(variable) {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|_| AppError::Config(format!("{variable} 必须是 1 到 65535 之间的端口号")))?,
        Err(env::VarError::NotPresent) => default_port,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(AppError::Config(format!("{variable} 必须是有效文本")));
        }
    };
    if port == 0 {
        return Err(AppError::Config(format!("{variable} 必须大于 0")));
    }
    probe_local_readiness(port)
}

/// 使用最小 HTTP/1.1 请求探测本机就绪端点，不依赖运行镜像中的外部工具。
pub fn probe_local_readiness(port: u16) -> Result<(), AppError> {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, HEALTHCHECK_TIMEOUT)
        .map_err(|error| healthcheck_error(port, "连接失败", error))?;
    stream
        .set_read_timeout(Some(HEALTHCHECK_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(HEALTHCHECK_TIMEOUT)))
        .map_err(|error| healthcheck_error(port, "设置超时失败", error))?;
    stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| healthcheck_error(port, "发送请求失败", error))?;

    let mut response = [0_u8; 64];
    let received = stream
        .read(&mut response)
        .map_err(|error| healthcheck_error(port, "读取响应失败", error))?;
    let status = String::from_utf8_lossy(&response[..received]);
    if status.starts_with("HTTP/1.1 200 ") || status.starts_with("HTTP/1.0 200 ") {
        return Ok(());
    }
    Err(AppError::Internal(format!(
        "本机就绪探测返回非 200 状态: 127.0.0.1:{port}"
    )))
}

fn healthcheck_error(port: u16, operation: &str, error: std::io::Error) -> AppError {
    AppError::Internal(format!(
        "本机就绪探测{operation}: 127.0.0.1:{port}: {error}"
    ))
}

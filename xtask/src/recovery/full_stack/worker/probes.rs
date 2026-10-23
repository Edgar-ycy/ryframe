use super::reject;
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::{
    net::{SocketAddr, TcpStream},
    thread,
    time::{Duration, Instant},
};

pub(super) fn execute(args: &Value) -> Result<Value> {
    strict::exact(args, &["action", "url", "timeout"], "Worker 端口探针")?;
    let raw = args["url"].as_str().ok_or("Worker 端点必须是字符串")?;
    let url = reqwest::Url::parse(raw)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || raw.contains(['\0', '\r', '\n'])
    {
        return reject("Worker 端点协议、凭据、查询或片段无效");
    }
    let timeout = args["timeout"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 180.0)
        .ok_or("Worker 端口等待时间无效")?;
    match args["action"].as_str() {
        Some("ready") => ready(url),
        Some("free" | "wait-free") => {
            let address = url
                .socket_addrs(|| None)?
                .into_iter()
                .next()
                .ok_or("Worker 端点没有网络地址")?;
            free(address, Duration::from_secs_f64(timeout))?;
            Ok(Value::Null)
        }
        _ => reject("未知 Worker 端口探针操作"),
    }
}

fn ready(url: reqwest::Url) -> Result<Value> {
    crate::http_session::install_crypto_provider()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(1))
            .build()?;
        let ready = tokio::select! {
            response = client.get(url).send() => response.is_ok_and(|r| r.status().as_u16() == 200),
            _ = tokio::signal::ctrl_c() => false,
        };
        Ok(json!(ready))
    })
}

fn free(address: SocketAddr, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        // 与旧 connect_ex 语义一致：只把连接成功认定为端口占用。
        if TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return reject(if timeout.is_zero() {
                "全栈就绪端口已占用，拒绝把其他进程当作新服务"
            } else {
                "全栈进程退出后就绪端口仍被占用"
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
}

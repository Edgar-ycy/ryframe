//! 提供已核验的前端产物，并将同源 API 请求转发给登记的恢复进程。
use std::{
    ffi::{OsStr, OsString},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::PathBuf,
};

use crate::{Result, recovery::frontend_build};

#[path = "frontend_site/content.rs"]
mod content;
#[path = "frontend_site/http.rs"]
mod http;
#[path = "frontend_site/peer.rs"]
mod peer;
#[path = "frontend_site/proxy.rs"]
mod proxy;

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/frontend_site.rs"]
mod tests;

pub(crate) const MARKER: &str = "__restore-frontend-serve";

struct Options {
    frontend: PathBuf,
    receipt: PathBuf,
    receipt_sha256: String,
    expected_head: String,
    address: SocketAddrV4,
    runtime_directory: PathBuf,
    api_port: u16,
    b0: bool,
}

impl Options {
    fn parse(args: &[OsString]) -> Result<Self> {
        let [
            frontend,
            root,
            receipt,
            file,
            receipt_sha,
            sha,
            head,
            expected,
            host,
            address,
            port,
            number,
            runtime_dir,
            directory,
            api_port,
            upstream,
            rest @ ..,
        ] = args
        else {
            return Err("前端静态服务必须提供完整的固定顺序参数".into());
        };
        let b0 = match rest {
            [] => false,
            [flag] if flag == "--b0" => true,
            _ => return Err("前端静态服务参数未知、重复或顺序无效".into()),
        };
        if frontend != "--frontend"
            || receipt != "--receipt"
            || receipt_sha != "--receipt-sha256"
            || head != "--expected-head"
            || host != "--host"
            || port != "--port"
            || runtime_dir != "--runtime-dir"
            || api_port != "--api-port"
            || address != "127.0.0.1"
        {
            return Err("前端静态服务参数顺序或本机地址无效".into());
        }
        let receipt_sha256 = exact_hex(sha, 64)?;
        let expected_head = exact_hex(expected, 40)?;
        if b0 && expected_head != frontend_build::b0::HEAD {
            return Err("B0 静态前端必须绑定固定旧前端 HEAD".into());
        }
        let port = exact_port(number)?;
        let api_port = exact_port(upstream)?;
        if api_port == port {
            return Err("前端和已登记 API 必须使用不同的端口".into());
        }
        Ok(Self {
            frontend: PathBuf::from(root),
            receipt: PathBuf::from(file),
            receipt_sha256,
            expected_head,
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            runtime_directory: PathBuf::from(directory),
            api_port,
            b0,
        })
    }
}

fn exact_port(value: &OsStr) -> Result<u16> {
    value
        .to_str()
        .filter(|text| text.as_bytes().first().is_some_and(|first| *first != b'0'))
        .filter(|text| text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| "恢复回环端口必须是 1 到 65535 的规范十进制".into())
}

fn exact_hex(value: &OsStr, length: usize) -> Result<String> {
    let value = value.to_str().ok_or("前端静态服务摘要必须是 UTF-8")?;
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("前端静态服务摘要必须是精确长度的小写十六进制".into());
    }
    Ok(value.to_owned())
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|arg| arg != MARKER) {
        return None;
    }
    Some(
        match Options::parse(&args[1..]).and_then(|options| serve(options, &args)) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("恢复前端静态服务失败：{error}");
                1
            }
        },
    )
}

fn prepare(options: &Options) -> Result<(TcpListener, content::Site)> {
    let site = content::Site::load(options)?;
    let listener = TcpListener::bind(options.address)?;
    listener.set_nonblocking(true)?;
    Ok((listener, site))
}

fn serve(options: Options, args: &[OsString]) -> Result<()> {
    use std::sync::Arc;

    let identity = peer::bind(&options, args)?;
    let upstream = Arc::new(proxy::Upstream::new(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, options.api_port),
        identity,
    )?);
    let (listener, site) = prepare(&options)?;
    let route = axum::routing::any(move |request: axum::extract::Request| {
        let upstream = Arc::clone(&upstream);
        async move { upstream.forward(request).await }
    });
    let app = http::router(site)
        .route("/api/v1", route.clone())
        .route("/api/v1/{*path}", route);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, app).await?;
        Ok(())
    })
}

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    time::Duration,
};

use crate::Result;

pub(crate) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: Vec<u8>,
}

pub(crate) fn request(
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<Response> {
    let (address, host, path) = loopback_url(url)?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("监控 HTTP 连接失败 {url}：{error}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let method = if body.is_some() { "POST" } else { "GET" };
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\nConnection: close\r\n");
    for (name, value) in headers {
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err("监控 HTTP 请求头无效".into());
        }
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes())?;
    if let Some(body) = body {
        stream.write_all(body)?;
    }
    stream.flush()?;
    read_response(&mut stream)
}

pub(crate) fn loopback_url(url: &str) -> Result<(SocketAddr, String, String)> {
    let (host, rest) = if let Some(value) = url.strip_prefix("http://127.0.0.1:") {
        ("127.0.0.1", value)
    } else if let Some(value) = url.strip_prefix("http://[::1]:") {
        ("::1", value)
    } else {
        return Err("监控 HTTP 只能访问明确的回环地址".into());
    };
    let split = rest.find('/').unwrap_or(rest.len());
    let port = rest[..split]
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("监控 HTTP 端点端口无效")?;
    let path = if split == rest.len() {
        "/".to_owned()
    } else {
        rest[split..].to_owned()
    };
    if path.contains(['\r', '\n', ' ']) {
        return Err("监控 HTTP 路径无效".into());
    }
    let address = if host == "::1" {
        SocketAddr::from((Ipv6Addr::LOCALHOST, port))
    } else {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    };
    let authority = if host == "::1" {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Ok((address, authority, path))
}

fn read_response(stream: &mut TcpStream) -> Result<Response> {
    let mut raw = Vec::new();
    let mut block = [0_u8; 16 * 1024];
    let end = loop {
        let read = stream.read(&mut block)?;
        if read == 0 {
            return Err("监控 HTTP 响应在头部结束前关闭".into());
        }
        raw.extend_from_slice(&block[..read]);
        if raw.len() > 64 * 1024 {
            return Err("监控 HTTP 响应头超过 64 KiB".into());
        }
        if let Some(index) = find(&raw, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = std::str::from_utf8(&raw[..end]).map_err(|_| "监控 HTTP 响应头不是 UTF-8")?;
    let mut lines = header.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or("监控 HTTP 响应状态无效")?;
    let mut content_type = String::new();
    let mut content_length = None;
    let mut chunked = false;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("监控 HTTP 响应头格式无效")?;
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-type") {
            content_type = value.to_owned();
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "监控 HTTP Content-Length 无效")?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case("chunked"))
        {
            chunked = true;
        }
    }
    let body = if chunked {
        read_chunked(&raw[end..], stream)?
    } else if let Some(length) = content_length {
        if length > MAX_RESPONSE_BYTES {
            return Err("监控 HTTP 响应超过 4 MiB".into());
        }
        read_length(&raw[end..], stream, length)?
    } else {
        let mut body = raw[end..].to_vec();
        let room = MAX_RESPONSE_BYTES + 1 - body.len().min(MAX_RESPONSE_BYTES);
        let mut remainder = Vec::new();
        stream.take(room as u64).read_to_end(&mut remainder)?;
        body.extend_from_slice(&remainder);
        if body.len() > MAX_RESPONSE_BYTES {
            return Err("监控 HTTP 响应超过 4 MiB".into());
        }
        body
    };
    Ok(Response {
        status,
        content_type,
        body,
    })
}

fn read_length(initial: &[u8], stream: &mut TcpStream, length: usize) -> Result<Vec<u8>> {
    let mut body = initial[..initial.len().min(length)].to_vec();
    if body.len() < length {
        let mut rest = vec![0_u8; length - body.len()];
        stream.read_exact(&mut rest)?;
        body.extend_from_slice(&rest);
    }
    Ok(body)
}

fn read_chunked(initial: &[u8], stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut raw = initial.to_vec();
    let mut offset = 0;
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(index) = find(&raw[offset..], b"\r\n") {
                break offset + index;
            }
            read_more(&mut raw, stream)?;
        };
        let size_text = std::str::from_utf8(&raw[offset..line_end])?;
        let size =
            usize::from_str_radix(size_text.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| "监控 HTTP chunk 长度无效")?;
        offset = line_end + 2;
        while raw.len() < offset.saturating_add(size).saturating_add(2) {
            read_more(&mut raw, stream)?;
        }
        if size == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(size) > MAX_RESPONSE_BYTES {
            return Err("监控 HTTP 响应超过 4 MiB".into());
        }
        body.extend_from_slice(&raw[offset..offset + size]);
        if &raw[offset + size..offset + size + 2] != b"\r\n" {
            return Err("监控 HTTP chunk 结尾无效".into());
        }
        offset = offset + size + 2;
    }
}

fn read_more(raw: &mut Vec<u8>, stream: &mut TcpStream) -> Result<()> {
    let mut block = [0_u8; 16 * 1024];
    let read = stream.read(&mut block)?;
    if read == 0 {
        return Err("监控 HTTP 响应提前结束".into());
    }
    raw.extend_from_slice(&block[..read]);
    if raw.len() > MAX_RESPONSE_BYTES + 64 * 1024 {
        return Err("监控 HTTP 响应超过 4 MiB".into());
    }
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

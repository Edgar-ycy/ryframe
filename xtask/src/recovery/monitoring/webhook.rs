use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::utc_timestamp::rfc3339_micros_z;
use crate::{
    Result,
    recovery::clone::native::{digest, json as json_io},
};

use super::contract::exact;

const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Protocol {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) events: PathBuf,
}

impl Protocol {
    pub(crate) fn encode(&self) -> Result<String> {
        Ok(serde_json::to_string(&json!({
            "events": self.events.to_string_lossy(),
            "host": self.host,
            "port": self.port,
        }))?)
    }

    pub(crate) fn decode(source: &str) -> Result<Self> {
        let value = json_io::parse_fragment(source, "监控 webhook 私有协议")?;
        let object = exact(&value, &["events", "host", "port"], "监控 webhook 私有协议")?;
        let host = object
            .get("host")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "127.0.0.1" | "::1"))
            .ok_or("监控 webhook 只能绑定明确 loopback 地址")?
            .to_owned();
        let port = object
            .get("port")
            .and_then(Value::as_u64)
            .filter(|value| (1024..=65535).contains(value))
            .and_then(|value| u16::try_from(value).ok())
            .ok_or("监控 webhook 端口无效")?;
        let events = object
            .get("events")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .filter(|value| value.is_absolute())
            .ok_or("监控 webhook 事件文件必须是绝对路径")?;
        Ok(Self { host, port, events })
    }
}

pub(crate) fn run(protocol: &Protocol) -> Result<()> {
    let metadata = fs::symlink_metadata(&protocol.events)?;
    if digest::linked(&metadata) || !metadata.is_file() || metadata.len() != 0 {
        return Err("监控 webhook 要求预先创建的空普通事件文件".into());
    }
    let mut stream = OpenOptions::new()
        .append(true)
        .read(true)
        .open(&protocol.events)?;
    let identity = file_identity(&metadata, &protocol.events)?;
    let address = if protocol.host == "::1" {
        SocketAddr::from((Ipv6Addr::LOCALHOST, protocol.port))
    } else {
        SocketAddr::from((Ipv4Addr::LOCALHOST, protocol.port))
    };
    let listener = TcpListener::bind(address)?;
    for connection in listener.incoming() {
        let mut connection = connection?;
        connection.set_read_timeout(Some(Duration::from_secs(5)))?;
        connection.set_write_timeout(Some(Duration::from_secs(5)))?;
        if let Err(error) = handle(&mut connection, &protocol.events, &mut stream, identity) {
            eprintln!("监控 webhook 请求失败：{error}");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity(u64, u64);

fn handle(
    stream: &mut TcpStream,
    events: &Path,
    output: &mut std::fs::File,
    identity: FileIdentity,
) -> Result<()> {
    let remote = stream.peer_addr()?.ip();
    if !remote.is_loopback() {
        return response(stream, 404, b"");
    }
    let request = read_request(stream)?;
    let (headers, body) = split_request(&request)?;
    let request_line = *headers.first().ok_or("监控 webhook 请求行缺失")?;
    let method = request_line.split_whitespace().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    if method == "GET" && path == "/readyz" {
        return response(stream, 200, br#"{"status":"ready"}"#);
    }
    if method != "POST" || path != "/alerts" {
        return response(stream, 404, b"");
    }
    if header(&headers, "content-type")
        .and_then(|value| value.split(';').next())
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return response(stream, 415, b"");
    }
    if body.is_empty() || body.len() > MAX_BODY {
        return response(stream, 413, b"");
    }
    let body = std::str::from_utf8(body).map_err(|_| "监控 webhook 请求体不是 UTF-8")?;
    let payload = json_io::parse_fragment(body, "Alertmanager webhook")?;
    if !payload.is_object() {
        return response(stream, 400, b"");
    }
    let value = json!({
        "received_at": rfc3339_micros_z(OffsetDateTime::now_utc()),
        "remote": remote.to_string(),
        "payload": payload,
    });
    let mut encoded = serde_json::to_vec(&value)?;
    encoded.push(b'\n');
    let before = output.metadata()?;
    if file_identity(&before, events)? != identity {
        return response(stream, 500, b"");
    }
    output.write_all(&encoded)?;
    output.sync_all()?;
    if file_identity(&fs::metadata(events)?, events)? != identity {
        return response(stream, 500, b"");
    }
    response(stream, 204, b"")
}

fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut block = [0_u8; 8192];
    loop {
        let read = stream.read(&mut block)?;
        if read == 0 {
            return Err("监控 webhook 请求提前结束".into());
        }
        raw.extend_from_slice(&block[..read]);
        if raw.len() > MAX_BODY + 64 * 1024 {
            return Err("监控 webhook 请求超过限制".into());
        }
        let Some(end) = raw.windows(4).position(|value| value == b"\r\n\r\n") else {
            if raw.len() > 64 * 1024 {
                return Err("监控 webhook 请求头超过 64 KiB".into());
            }
            continue;
        };
        let headers = std::str::from_utf8(&raw[..end])?;
        let length = content_length(headers)?;
        if length == 0 {
            return Ok(raw);
        }
        if raw.len() >= end + 4 + length {
            return Ok(raw);
        }
    }
}

fn split_request(raw: &[u8]) -> Result<(Vec<&str>, &[u8])> {
    let end = raw
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .ok_or("监控 webhook 请求头未结束")?;
    let headers = std::str::from_utf8(&raw[..end])?.lines().collect();
    Ok((headers, &raw[end + 4..]))
}

fn content_length(headers: &str) -> Result<usize> {
    let values = headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(0);
    }
    if values.len() != 1 {
        return Err("监控 webhook 必须提供唯一 Content-Length".into());
    }
    let length = values[0]
        .parse::<usize>()
        .map_err(|_| "监控 webhook Content-Length 无效")?;
    if length > MAX_BODY {
        return Err("监控 webhook 请求体大小无效".into());
    }
    Ok(length)
}

fn header<'a>(headers: &'a [&str], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .filter_map(|line| line.split_once(':'))
        .find(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

fn response(stream: &mut TcpStream, status: u16, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn file_identity(metadata: &fs::Metadata, path: &Path) -> Result<FileIdentity> {
    let (device, inode) =
        crate::recovery::clone::native::resources::common::device_inode_for(path, metadata)?;
    Ok(FileIdentity(device, inode))
}

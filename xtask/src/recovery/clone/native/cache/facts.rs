use super::{
    super::{digest, json as document},
    model,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    time::Duration,
};

pub(super) fn bounded(backend: &std::path::Path, request: &Value) -> Result<Value> {
    // 保留原 facts 单次有界调用的30s期限，防止内联后按每次读写重新计时。
    let input = serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-cache-process",
        "operation":"facts","arguments":{"request":request}}))?;
    if input.len() > 1024 * 1024 {
        return Err("缓存完整来源请求超过1MiB".into());
    }
    let executable = std::env::current_exe()?;
    let output = crate::process::command_output_with_input_timeout(
        backend,
        executable.to_str().ok_or("缓存事实工具路径不是UTF-8")?,
        &["__cache-process"],
        &[],
        Some(&input),
        Duration::from_secs(30),
    )?;
    if output.stdout.len() > 1024 * 1024 {
        return Err("缓存事实响应超限".into());
    }
    let value = document::parse_fragment(std::str::from_utf8(&output.stdout)?, "缓存事实响应")?;
    if !output.status.success() || value["ok"] != true {
        if value["error"] == "CacheConnectionRefused" {
            return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
        }
        return Err("缓存事实读取未完成；保留现场".into());
    }
    document::exact(&value, &["ok", "result"], "缓存事实成功响应")?;
    Ok(value["result"].clone())
}

pub(super) fn observe(request: &Value) -> Result<Value> {
    let password = model::password(&request["password"])?;
    let port = request["port"]
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .filter(|v| *v >= 1024)
        .ok_or("Redis 端口无效")?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_secs(3),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut send = |parts: &[&str]| -> Result<Value> {
        let mut command = format!("*{}\r\n", parts.len()).into_bytes();
        for part in parts {
            command.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
            command.extend_from_slice(part.as_bytes());
            command.extend_from_slice(b"\r\n");
        }
        stream.write_all(&command)?;
        response(&mut reader, 0)
    };
    let (facts, actual) = read_facts(request, password, &mut send)?;
    Ok(
        json!({"process_id":facts.get("process_id"),"run_id":facts.get("run_id"),"config_file":facts.get("config_file"),"configuration_sha256":digest::plan_hash(&actual)?}),
    )
}

fn read_facts(
    request: &Value,
    password: &str,
    send: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<(BTreeMap<String, String>, Value)> {
    if send(&["AUTH", password])? != "OK" {
        return Err("Redis 原凭据认证未通过".into());
    }
    let raw = send(&["INFO", "server"])?;
    let mut facts = BTreeMap::new();
    for line in document::text(&raw, "Redis INFO")?.lines() {
        if !line.starts_with('#')
            && let Some((key, value)) = line.split_once(':')
            && facts.insert(key.to_owned(), value.to_owned()).is_some()
        {
            return Err("Redis INFO 重复字段".into());
        }
    }
    let actual = model::configuration(request)?;
    for (key, value) in actual.as_object().unwrap() {
        if send(&["CONFIG", "GET", key])? != json!([key, value]) {
            return Err("Redis 实际目录、持久化或监听配置变化".into());
        }
    }
    Ok((facts, actual))
}

pub(super) fn check(
    request: &Value,
    identity: &Value,
    facts: &Value,
    run_id: &Value,
) -> Result<()> {
    let pid = identity["pid"]
        .as_u64()
        .ok_or("Redis 内核PID无效")?
        .to_string();
    if facts["process_id"] != pid
        || facts["config_file"]
            != model::linux_path(document::text(
                &request["configuration"]["path"],
                "Redis 配置",
            )?)?
        || !model::matches(&facts["run_id"], r"^[a-f0-9]{40}$")?
        || facts["run_id"] == request["previous_run_id"]
        || !run_id.is_null() && facts["run_id"] != *run_id
        || facts["configuration_sha256"] != digest::plan_hash(&model::configuration(request)?)?
    {
        return Err("Redis INFO、运行代次和实际配置不属于本次进程".into());
    }
    Ok(())
}

pub(super) fn response(reader: &mut impl BufRead, depth: usize) -> Result<Value> {
    if depth > 2 {
        return Err("Redis 响应嵌套过深".into());
    }
    let mut bytes = Vec::new();
    reader.take(1048577).read_until(b'\n', &mut bytes)?;
    if bytes.len() > 1048576 || !bytes.ends_with(b"\r\n") || bytes.len() < 3 {
        return Err("Redis 响应截断或超界".into());
    }
    let value =
        std::str::from_utf8(&bytes[1..bytes.len() - 2]).map_err(|_| "Redis 响应不是UTF-8")?;
    match bytes[0] {
        b'+' => Ok(json!(value)),
        b'$' => {
            let count = value
                .parse::<usize>()
                .ok()
                .filter(|v| *v <= 1048576)
                .ok_or("Redis 字符串响应长度无效")?;
            let mut body = vec![0; count + 2];
            reader.read_exact(&mut body)?;
            if !body.ends_with(b"\r\n") {
                return Err("Redis 字符串响应不完整".into());
            }
            Ok(json!(
                std::str::from_utf8(&body[..count]).map_err(|_| "Redis 字符串不是UTF-8")?
            ))
        }
        b'*' => {
            let count = value
                .parse::<usize>()
                .ok()
                .filter(|v| *v <= 32)
                .ok_or("Redis 配置响应条目过多")?;
            (0..count)
                .map(|_| response(reader, depth + 1))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array)
        }
        _ => Err("Redis 认证或固定核验命令失败".into()),
    }
}

#[cfg(test)]
pub(super) fn observe_with(
    request: &Value,
    send: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<Value> {
    let (facts, actual) = read_facts(request, model::password(&request["password"])?, send)?;
    Ok(
        json!({"process_id":facts.get("process_id"),"run_id":facts.get("run_id"),"config_file":facts.get("config_file"),"configuration_sha256":digest::plan_hash(&actual)?}),
    )
}

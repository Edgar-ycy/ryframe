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
    bounded_call(backend, "facts", json!({"request":request}))
}

#[cfg(windows)]
pub(super) fn bounded_windows(
    backend: &std::path::Path,
    request: &Value,
    identity: &Value,
) -> Result<Value> {
    bounded_call(
        backend,
        "windows-facts",
        json!({"request":request,"identity":identity}),
    )
}

fn bounded_call(backend: &std::path::Path, operation: &str, arguments: Value) -> Result<Value> {
    // 保留原 facts 单次有界调用的30s期限，防止内联后按每次读写重新计时。
    let input = serde_json::to_vec(&json!({"format_version":1,"kind":"ryframe-cache-process",
        "operation":operation,"arguments":arguments}))?;
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
    if request["format_version"] == 3 {
        return Err("Windows Redis 事实核验需要登记进程身份".into());
    }
    let password = model::password(&request["password"])?;
    read_connection(request, password, &connect(request)?)
}

#[cfg(windows)]
pub(super) fn observe_windows(request: &Value, identity: &Value) -> Result<Value> {
    use crate::recovery::fixture_control::services::lifecycle::readonly::process;

    if request["format_version"] != 3 {
        return Err("Windows Redis 事实核验只接受 v3 请求".into());
    }
    model::shape(request)?;
    let password = model::password(&request["password"])?;
    let stream = connect(request)?;
    process::require_loopback_peer(identity, &stream)?;
    let observed = read_connection(request, password, &stream);
    match (observed, process::require_loopback_peer(identity, &stream)) {
        (Ok(facts), Ok(())) => Ok(facts),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => {
            Err(format!("Redis 事实读取失败：{error}；连接身份后验失败：{after}").into())
        }
    }
}

#[cfg(windows)]
pub(super) fn shutdown_windows(
    backend: &std::path::Path,
    request: &Value,
    identity: &Value,
    expected: &Value,
    run_id: &Value,
    stop_deadline: std::time::Instant,
    sent: &mut bool,
) -> Result<()> {
    use crate::recovery::fixture_control::services::lifecycle::readonly::process;

    let deadline = stop_deadline.min(std::time::Instant::now() + Duration::from_secs(30));
    if *sent || request["format_version"] != 3 {
        return Err("Windows Redis 停止只接受原 v3 代次的首次尝试".into());
    }
    model::validate(backend, request, true)?;
    let password = model::password(&request["password"])?;
    let mut stream = connect(request)?;
    process::require_loopback_peer(identity, &stream)?;
    let observed = read_connection(request, password, &stream)?;
    check(request, identity, &observed, run_id)?;
    if observed != *expected {
        return Err("停止前 Windows Redis 认证事实与原登记不同".into());
    }
    process::require_loopback_peer(identity, &stream)?;
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
    }
    stream.set_write_timeout(Some(remaining.min(Duration::from_secs(3))))?;
    *sent = true;
    write_command(&mut stream, &["SHUTDOWN", "NOSAVE"])?;
    // 仅发送一次；由同一监督器的 Job 退出收据证明停止结果。
    Ok(())
}

fn connect(request: &Value) -> Result<TcpStream> {
    let port = request["port"]
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .filter(|v| *v >= 1024)
        .ok_or("Redis 端口无效")?;
    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_secs(3),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(stream)
}

fn read_connection(request: &Value, password: &str, mut stream: &TcpStream) -> Result<Value> {
    let mut reader = BufReader::new(stream);
    let mut send = |parts: &[&str]| -> Result<Value> {
        write_command(&mut stream, parts)?;
        response(&mut reader, 0)
    };
    let (facts, actual) = read_facts(request, password, &mut send)?;
    Ok(
        json!({"process_id":facts.get("process_id"),"run_id":facts.get("run_id"),"config_file":facts.get("config_file"),"configuration_sha256":digest::plan_hash(&actual)?}),
    )
}

fn write_command(stream: &mut impl Write, parts: &[&str]) -> std::io::Result<()> {
    let mut command = format!("*{}\r\n", parts.len()).into_bytes();
    for part in parts {
        command.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
        command.extend_from_slice(part.as_bytes());
        command.extend_from_slice(b"\r\n");
    }
    stream.write_all(&command)
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
    let pid_matches = if request["format_version"] == 3 {
        model::shape(request)?;
        // Windows 便携 Redis 报告 Cygwin PID；调用方用内核身份和监听者绑定同一进程。
        facts["process_id"].as_str().is_some_and(|text| {
            text.bytes().all(|byte| byte.is_ascii_digit())
                && text.parse::<u32>().is_ok_and(|pid| pid > 0)
        })
    } else {
        let pid = identity["pid"].as_u64().ok_or("Redis 内核PID无效")?;
        facts["process_id"] == pid.to_string()
    };
    if !pid_matches
        || facts["config_file"] != model::configuration_file(request)?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_info_uses_redis_pid_and_exact_path_and_run_id() {
        let request = model::tests::windows_request();
        let run_id = json!("b".repeat(40));
        let identity = &request["previous_identity"];
        let mut facts = json!({"process_id":"1457","run_id":run_id,
            "config_file":"/cygdrive/d/缓存 数据/redis.conf",
            "configuration_sha256":digest::plan_hash(&model::configuration(&request).unwrap()).unwrap()});
        check(&request, identity, &facts, &run_id).unwrap();
        for invalid in [
            json!(1457),
            json!(""),
            json!("0"),
            json!("-1"),
            json!("+1"),
            json!("1 2"),
        ] {
            facts["process_id"] = invalid;
            assert!(check(&request, identity, &facts, &run_id).is_err());
        }
        facts["process_id"] = json!("1457");
        facts["config_file"] = json!("/mnt/d/缓存 数据/redis.conf");
        assert!(check(&request, identity, &facts, &run_id).is_err());
        facts["config_file"] = json!("/cygdrive/d/缓存 数据/redis.conf");
        assert!(check(&request, identity, &facts, &json!("c".repeat(40))).is_err());
        facts["run_id"] = request["previous_run_id"].clone();
        assert!(check(&request, identity, &facts, &Value::Null).is_err());
        facts["run_id"] = run_id.clone();
        facts["configuration_sha256"] = json!("d".repeat(64));
        assert!(check(&request, identity, &facts, &run_id).is_err());
    }

    #[test]
    fn linux_info_still_requires_the_kernel_pid() {
        let request = super::super::tests::request();
        let run_id = json!("b".repeat(40));
        let identity = &request["previous_identity"];
        let mut facts = json!({"process_id":"200","run_id":run_id,
            "config_file":"/mnt/d/data/redis.conf",
            "configuration_sha256":digest::plan_hash(&model::configuration(&request).unwrap()).unwrap()});
        check(&request, identity, &facts, &run_id).unwrap();
        facts["process_id"] = json!("1457");
        assert!(check(&request, identity, &facts, &run_id).is_err());
    }
}

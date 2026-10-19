use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use serde_json::{Value, json};

use super::input::TargetContext;
use super::storage::StorageObserver;
use crate::Result;

pub(crate) fn verify_with_observer(
    context: &TargetContext,
    redis: &Value,
    observer: &impl StorageObserver,
) -> Result<Value> {
    let mut command = |parts: &[&str]| observer.redis(context, redis, parts);
    let scope = context.target_scope()?;
    let namespace = format!("ryframe:{{{scope}}}:");
    let ownership_key = format!("{namespace}.ryframe-owner");
    let sentinel_key = context
        .reset()
        .get("sentinel_key")
        .and_then(Value::as_str)
        .ok_or("目标 Redis sentinel key 无效")?;
    let sentinel_value = context
        .reset()
        .get("sentinel_value")
        .and_then(Value::as_str)
        .ok_or("目标 Redis sentinel value 无效")?;
    let keys = scan(&mut command, &namespace)?;
    let owner = get(&mut command, &ownership_key)?;
    let sentinel = get(&mut command, sentinel_key)?;
    let info = command(&["INFO", "server"])?;
    let info = info.as_str().ok_or("目标 Redis INFO 响应不是字符串")?;
    let storage = redis;
    let expected_pid = storage
        .get("pid")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let expected_run = storage
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut actual_pid = None;
    let mut actual_run = None;
    let mut fields = BTreeSet::new();
    for line in info.lines() {
        if !line.starts_with('#')
            && let Some((key, _)) = line.split_once(':')
            && !fields.insert(key)
        {
            return Err("目标 Redis INFO 字段重复".into());
        }
        if let Some(value) = line.strip_prefix("process_id:") {
            actual_pid = value.parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("run_id:") {
            actual_run = Some(value);
        }
    }
    let expected_owner = format!("ryframe-owner:v1:{scope}:redis");
    if keys != BTreeSet::from([ownership_key])
        || owner.as_deref() != Some(expected_owner.as_str())
        || sentinel.as_deref() != Some(sentinel_value)
        || actual_pid != Some(expected_pid)
        || actual_run != Some(expected_run)
    {
        return Err("目标 Redis namespace、owner 或 sentinel 与初始化不同".into());
    }
    Ok(json!({
        "keys": keys.into_iter().collect::<Vec<_>>(),
        "owner": owner,
        "sentinel": sentinel,
    }))
}

pub(super) struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    pub(super) fn authenticated(context: &TargetContext, redis: &Value) -> Result<Self> {
        let port = redis
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or("目标 Redis 端口无效")?;
        let password = context
            .environment
            .get("APP_REDIS_PASSWORD")
            .filter(|value| !value.is_empty())
            .ok_or("目标 Redis 缺少显式密码")?;
        let mut client = Self::connect(port)?;
        auth(&mut client, password)?;
        Ok(client)
    }

    fn connect(port: u16) -> Result<Self> {
        let address = ("127.0.0.1", port)
            .to_socket_addrs()?
            .next()
            .ok_or("无法解析目标 Redis 回环地址")?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { stream, reader })
    }

    pub(super) fn command(&mut self, parts: &[&str]) -> Result<Value> {
        let mut request = format!("*{}\r\n", parts.len());
        for part in parts {
            request.push_str(&format!("${}\r\n{}\r\n", part.len(), part));
        }
        self.stream.write_all(request.as_bytes())?;
        self.stream.flush()?;
        self.response(0)
    }

    fn response(&mut self, depth: usize) -> Result<Value> {
        if depth > 8 {
            return Err("目标 Redis 响应嵌套过深".into());
        }
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 || !line.ends_with("\r\n") || line.len() < 3 {
            return Err("目标 Redis 响应不完整".into());
        }
        let line = line.trim_end_matches("\r\n");
        let (kind, value) = line.split_at(1);
        match kind {
            "+" => Ok(Value::String(value.to_owned())),
            "-" => Err("目标 Redis 认证或固定核验命令失败".into()),
            ":" => Ok(Value::String(value.to_owned())),
            "$" => {
                let length = value
                    .parse::<i64>()
                    .map_err(|_| "目标 Redis bulk 长度无效")?;
                if length == -1 {
                    return Ok(Value::Null);
                }
                if !(0..=16 * 1024 * 1024).contains(&length) {
                    return Err("目标 Redis bulk 长度越界".into());
                }
                let mut body = vec![0; length as usize + 2];
                self.reader.read_exact(&mut body)?;
                if !body.ends_with(b"\r\n") {
                    return Err("目标 Redis bulk 截断".into());
                }
                body.truncate(length as usize);
                String::from_utf8(body)
                    .map(Value::String)
                    .map_err(|_| "目标 Redis bulk 不是 UTF-8".into())
            }
            "*" => {
                let count = value
                    .parse::<i64>()
                    .map_err(|_| "目标 Redis 数组长度无效")?;
                if !(0..=10_000).contains(&count) {
                    return Err("目标 Redis 数组长度越界".into());
                }
                (0..count)
                    .map(|_| self.response(depth + 1))
                    .collect::<Result<Vec<_>>>()
                    .map(Value::Array)
            }
            _ => Err("目标 Redis 响应类型未知".into()),
        }
    }
}

fn auth(client: &mut Client, password: &str) -> Result<()> {
    if client.command(&["AUTH", password])? != Value::String("OK".into())
        || client.command(&["SELECT", "0"])? != Value::String("OK".into())
    {
        return Err("目标 Redis 认证或 database 0 选择失败".into());
    }
    Ok(())
}

fn scan(
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
    prefix: &str,
) -> Result<BTreeSet<String>> {
    let mut cursor = "0".to_owned();
    let mut seen = BTreeSet::new();
    let mut cursors = BTreeSet::new();
    let pattern = format!("{prefix}*");
    for _ in 0..10_000 {
        let value = command(&["SCAN", &cursor, "MATCH", &pattern, "COUNT", "1000"])?;
        let page = value.as_array().ok_or("目标 Redis SCAN 响应不是数组")?;
        if page.len() != 2
            || !page[0].is_string()
            || !page[1].is_array()
            || page[0].as_str() == Some("")
            || !page[0]
                .as_str()
                .unwrap_or_default()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return Err("目标 Redis SCAN 分页格式无效".into());
        }
        for key in page[1].as_array().unwrap() {
            let key = key.as_str().ok_or("目标 Redis key 不是字符串")?;
            if !key.starts_with(prefix) || !seen.insert(key.to_owned()) {
                return Err("目标 Redis key 越界或重复".into());
            }
        }
        cursor = page[0].as_str().unwrap().to_owned();
        if cursor == "0" {
            return Ok(seen);
        }
        if !cursors.insert(cursor.clone()) {
            return Err("目标 Redis SCAN 游标循环".into());
        }
    }
    Err("目标 Redis SCAN 超过有界限制".into())
}

fn get(command: &mut impl FnMut(&[&str]) -> Result<Value>, key: &str) -> Result<Option<String>> {
    let value = command(&["GET", key])?;
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err("目标 Redis GET 响应不是字符串或 NULL".into()),
    }
}

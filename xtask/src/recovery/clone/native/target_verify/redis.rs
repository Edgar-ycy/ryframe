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
    let inputs = context.storage_inputs();
    let mut command = |parts: &[&str]| observer.redis(&inputs, redis, parts);
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

/// prepare 只接受完整空 namespace 和不存在的 owner/sentinel，不把协议错误当成缺失。
pub(crate) fn absence_with_observer(
    inputs: &super::storage::StorageInputs<'_>,
    redis: &Value,
    observer: &impl StorageObserver,
) -> Result<Value> {
    state_with_observer(inputs, redis, false, false, observer)
}

pub(crate) fn state_with_observer(
    inputs: &super::storage::StorageInputs<'_>,
    redis: &Value,
    initialized: bool,
    sentinel: bool,
    observer: &impl StorageObserver,
) -> Result<Value> {
    state(inputs, initialized, sentinel, &mut |parts| {
        observer.redis(inputs, redis, parts)
    })
}

pub(super) fn state(
    inputs: &super::storage::StorageInputs<'_>,
    initialized: bool,
    sentinel: bool,
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<Value> {
    let scope = super::super::json::text(&inputs.request["target"]["scope_id"], "Redis scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    let owner_key = format!("{namespace}.ryframe-owner");
    let keys = scan(command, &namespace)?;
    let owner = get(command, &owner_key)?;
    let marker = get(
        command,
        super::super::json::text(&inputs.request["reset"]["sentinel_key"], "Redis sentinel")?,
    )?;
    let expected_keys = if initialized {
        BTreeSet::from([owner_key])
    } else {
        BTreeSet::new()
    };
    let expected_owner = initialized.then(|| format!("ryframe-owner:v1:{scope}:redis"));
    let expected_marker = if sentinel {
        Some(
            super::super::json::text(
                &inputs.request["reset"]["sentinel_value"],
                "Redis sentinel值",
            )?
            .to_owned(),
        )
    } else {
        None
    };
    if keys != expected_keys || owner != expected_owner || marker != expected_marker {
        return Err("目标Redis namespace、owner或sentinel不同于精确前后像".into());
    }
    Ok(json!({"keys":keys,"owner":owner,"sentinel":marker}))
}

pub(crate) struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    pub(crate) fn authenticated(
        context: &super::storage::StorageInputs<'_>,
        redis: &Value,
    ) -> Result<Self> {
        let port = redis
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or("目标 Redis 端口无效")?;
        let root = context.execution_backend()?;
        let (directory, files) = super::input::request_configuration::fresh_files(
            &root,
            context.environment,
            context.request,
        )?;
        if super::input::request_configuration::configuration_identity(&directory, &files)?
            != context.request["configuration_sha256"]
                .as_str()
                .unwrap_or_default()
        {
            return Err("目标 Redis 配置文件与登记摘要不同；请重新审阅请求".into());
        }
        let table = crate::rate_limit::load_app_table(&root, Some(&directory), "test")
            .map_err(|_| "目标 Redis 配置无法加载；请检查所选 TOML 文件")?;
        let redis_config = table
            .get("redis")
            .and_then(toml::Value::as_table)
            .ok_or("目标配置缺少 redis 表")?;
        if redis_config.get("host").and_then(toml::Value::as_str) != Some("127.0.0.1")
            || redis_config.get("port").and_then(toml::Value::as_integer) != Some(i64::from(port))
            || redis_config
                .get("database")
                .and_then(toml::Value::as_integer)
                != Some(0)
            || redis_config.get("tls").and_then(toml::Value::as_bool) != Some(false)
        {
            return Err("目标 redis.host/port/database/tls 与登记端点不同；请检查绑定 TOML".into());
        }
        let password = redis_config
            .get("password")
            .and_then(toml::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("目标 TOML 缺少 redis.password；请显式配置密码")?;
        if super::input::request_configuration::fresh_files(
            &root,
            context.environment,
            context.request,
        )? != (directory, files)
        {
            return Err("目标 Redis 配置读取期间变化".into());
        }
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
        self.exchange(parts, false)
    }

    pub(crate) fn transaction_command(&mut self, parts: &[&str]) -> Result<Value> {
        self.exchange(parts, true)
    }

    fn exchange(&mut self, parts: &[&str], integers: bool) -> Result<Value> {
        let mut request = format!("*{}\r\n", parts.len());
        for part in parts {
            request.push_str(&format!("${}\r\n{}\r\n", part.len(), part));
        }
        self.stream.write_all(request.as_bytes())?;
        self.stream.flush()?;
        self.response(0, integers, integers && parts == ["EXEC"])
    }

    fn response(&mut self, depth: usize, integers: bool, exec: bool) -> Result<Value> {
        if depth > 8 {
            return Err("目标 Redis 响应嵌套过深".into());
        }
        let mut line = String::new();
        let count = if integers {
            (&mut self.reader).take(1_048_577).read_line(&mut line)?
        } else {
            self.reader.read_line(&mut line)?
        };
        if count == 0
            || !line.ends_with("\r\n")
            || line.len() < 3
            || integers && line.len() > 1_048_576
        {
            return Err("目标 Redis 响应不完整或超限".into());
        }
        let line = line.trim_end_matches("\r\n");
        let (kind, value) = line.split_at_checked(1).ok_or("目标 Redis 响应前缀无效")?;
        if exec && kind != "*" {
            return Err("目标 Redis EXEC 必须返回数组或被 WATCH 中止的 nil 数组".into());
        }
        match kind {
            "+" => Ok(Value::String(value.to_owned())),
            "-" => Err("目标 Redis 认证或固定核验命令失败".into()),
            ":" if integers => value
                .parse::<i64>()
                .map(|value| json!(value))
                .map_err(|_| "目标 Redis 整数响应无效".into()),
            ":" => Ok(Value::String(value.to_owned())),
            "$" => {
                let length = value
                    .parse::<i64>()
                    .map_err(|_| "目标 Redis bulk 长度无效")?;
                if length == -1 {
                    return Ok(Value::Null);
                }
                let limit = if integers {
                    1024 * 1024
                } else {
                    16 * 1024 * 1024
                };
                if !(0..=limit).contains(&length) {
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
                if count == -1 && integers {
                    return Ok(Value::Null);
                }
                if !(0..=10_000).contains(&count) {
                    return Err("目标 Redis 数组长度越界".into());
                }
                (0..count)
                    .map(|_| self.response(depth + 1, integers, false))
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

pub(super) fn scan(
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

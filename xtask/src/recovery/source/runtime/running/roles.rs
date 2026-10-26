//! 源 API/Worker 使用原进程树格式，不把 RustFS 专用 reader 改成宽泛角色入口。
use crate::recovery::fixture_control::services::lifecycle::readonly::process as observation;
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};

use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common},
};
use serde_json::{Value, json};

pub(in crate::recovery::source::evidence) fn directory(
    runtime: &Path,
    operations: &Value,
) -> Result<()> {
    document::exact(operations, &["api", "worker"], "源角色操作集合")?;
    let mut allowed = BTreeSet::from(["binaries.json".to_owned(), "runtime.json".to_owned()]);
    let mut ids = BTreeSet::new();
    for role in ["api", "worker"] {
        let operation = operation(&operations[role])?;
        if !ids.insert(operation) {
            return Err("源角色操作标识不能重复".into());
        }
        allowed.extend([
            format!("{role}.log"),
            format!("{role}.json"),
            format!("{role}-tree.json"),
            format!("{role}-members-{operation}-ready.json"),
            format!("{role}-members-{operation}-stopped.json"),
            format!("{role}-tree-{operation}-control.json"),
            format!("{role}-tree-{operation}-result.json"),
        ]);
    }
    for entry in std::fs::read_dir(runtime)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "源进程树文件名必须为 UTF-8")?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !allowed.contains(&name) || !metadata.is_file() || digest::linked(&metadata) {
            return Err("源进程树目录包含未知、链接或非普通文件".into());
        }
    }
    Ok(())
}

pub(in crate::recovery::source::evidence) fn process(
    runtime: &Path,
    role: &str,
    scope: &Value,
) -> Result<Value> {
    let value = read(&runtime.join(format!("{role}.json")))?;
    document::exact(
        &value,
        &["format_version", "role", "scope_id", "identity"],
        "源进程收据",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["role"] != role
        || value["scope_id"] != *scope
    {
        return Err("源进程收据不属于同一角色及隔离 scope".into());
    }
    common::validate_process_identity(&value["identity"])
}

pub(in crate::recovery::source::evidence) fn tree(
    runtime: &Path,
    role: &str,
    scope: &Value,
) -> Result<Value> {
    let value = read(&runtime.join(format!("{role}-tree.json")))?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "role",
            "scope_id",
            "operation_id",
            "supervisor",
            "process",
            "monitor",
            "group_id",
        ],
        "源进程树",
    )?;
    if value["format_version"].as_u64() != Some(2)
        || value["kind"] != "full-stack-process-tree"
        || value["runtime_directory"] != json!(runtime)
        || value["role"] != role
        || value["scope_id"] != *scope
        || value["group_id"] != value["supervisor"]["pid"]
        || [
            value["supervisor"]["pid"].clone(),
            value["process"]["pid"].clone(),
        ]
        .contains(&value["monitor"]["pid"])
    {
        return Err("源进程树缺少同一角色、scope 或可信进程组身份".into());
    }
    operation(&value["operation_id"])?;
    for field in ["supervisor", "process", "monitor"] {
        common::validate_process_identity(&value[field])?;
    }
    Ok(value)
}

fn operation(value: &Value) -> Result<&str> {
    let text = document::text(value, "源进程树操作标识")?;
    if text.len() != 32
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("源进程树操作标识必须是 32 位小写十六进制".into());
    }
    Ok(text)
}

fn read(path: &Path) -> Result<Value> {
    let before = common::binding(path)?;
    if before["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes > 64 * 1024)
    {
        return Err("源进程树收据超过 64KiB".into());
    }
    let value = document::read(path)?;
    if common::binding(path)? != before {
        return Err("源进程树收据读取期间变化".into());
    }
    Ok(value)
}

pub(super) fn ready(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|_| "源 ready URL 无效")?;
    common::endpoint(url)?;
    if parsed.path() != "/readyz" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("源 ready 观察必须使用精确 /readyz".into());
    }
    let address = SocketAddr::new(
        parsed
            .host_str()
            .and_then(|s| s.parse::<IpAddr>().ok())
            .ok_or("源 ready 必须绑定明确 IP")?,
        parsed.port_or_known_default().ok_or("源 ready 缺少端口")?,
    );
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .map_err(|error| ready_io(address, "连接", &error))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| ready_io(address, "设置读取期限", &error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| ready_io(address, "设置写入期限", &error))?;
    write!(
        stream,
        "GET /readyz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        address
    )
    .map_err(|error| ready_io(address, "发送只读 ready 请求", &error))?;
    let mut line = Vec::new();
    std::io::Read::take(BufReader::new(stream), 65537)
        .read_until(b'\n', &mut line)
        .map_err(|error| ready_io(address, "读取 HTTP 状态", &error))?;
    if line.len() > 65536 || !line.ends_with(b"\n") {
        return Err(format!("源 ready {address} 状态行缺失、截断或超过 HTTP 状态行上限；检查绑定服务的 HTTP 响应，保留现场，不重试").into());
    }
    let status = std::str::from_utf8(&line).map_err(|_| "源 ready 状态行无效")?;
    let mut fields = status.split_whitespace();
    let protocol = fields.next();
    let code = fields
        .next()
        .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<u16>().ok());
    if !matches!(protocol, Some("HTTP/1.0" | "HTTP/1.1"))
        || code.is_none_or(|code| !(100..=999).contains(&code))
    {
        return Err(format!(
            "源 ready {address} 返回无效 HTTP 协议或状态码；检查绑定服务协议，保留现场，不重试"
        )
        .into());
    }
    if code != Some(200) {
        return Err(format!("源 ready {address} 返回 HTTP {}，要求200；检查 API/Worker 就绪状态和对应服务日志，保留现场，不重试",code.ok_or("ready 状态码缺失")?).into());
    }
    Ok(())
}

pub(super) fn observe(
    runtime: &Path,
    evidence: &Value,
    intent: &Value,
    selected: &Value,
    contract: &Value,
    live: bool,
) -> Result<()> {
    document::exact(&evidence["roles"], &["api", "worker"], "运行中源角色")?;
    for role in ["api", "worker"] {
        let row = &evidence["roles"][role];
        document::exact(row, &["process", "tree", "ready"], "运行中源角色证据")?;
        let identity = process(runtime, role, &selected["scope_id"])?;
        let tree = tree(runtime, role, &selected["scope_id"])?;
        if row["process"] != common::binding(&runtime.join(format!("{role}.json")))?
            || row["tree"] != common::binding(&runtime.join(format!("{role}-tree.json")))?
            || row["ready"] != true
            || tree["operation_id"] != intent["operations"][role]
            || tree["process"] != identity
        {
            return Err("运行中源进程树、创建身份或启动意图变化".into());
        }
        if live {
            for field in ["process", "supervisor", "monitor"] {
                observation::require(&tree[field])?;
            }
            let url = if role == "api" {
                format!(
                    "{}/readyz",
                    document::text(&selected["api_url"], "源 API")?.trim_end_matches('/')
                )
            } else {
                document::text(&contract["worker_ready_url"], "源 Worker ready")?.to_owned()
            };
            observation::listener(&identity, &url)?;
            ready(&url)?;
        }
    }
    Ok(())
}

fn ready_io(address: SocketAddr, stage: &str, error: &std::io::Error) -> String {
    format!(
        "源 ready {address} 在{stage}阶段失败：{error}（类别 {:?}，OS码 {:?}）；检查绑定端口、服务进程和本机网络权限，保留现场，不启动或重试",
        error.kind(),
        error.raw_os_error()
    )
}

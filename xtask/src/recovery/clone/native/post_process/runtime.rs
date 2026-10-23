use std::{
    net::{IpAddr, SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{json as json_io, resources::common};
use crate::Result;

pub(super) fn proof(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &["backend", "directory", "scope_id", "api_url"],
        "运行来源核验请求",
    )?;
    let backend = path(&root["backend"], "后端目录")?;
    let directory = path(&root["directory"], "运行目录")?;
    if std::env::var("APP_ENV").as_deref() != Ok("test") {
        return Err("运行来源核验只允许 APP_ENV=test".into());
    }
    let canonical = backend.canonicalize()?;
    if canonical != backend {
        return Err("后端目录必须是规范绝对路径".into());
    }
    let directory = common::local_directory(&backend, &directory, "运行目录")?;
    let runtime = contract(&backend, &directory)?;
    let scope = json_io::text(&root["scope_id"], "运行 scope")?;
    if runtime["scope_id"] != scope {
        return Err("运行来源 scope 不匹配".into());
    }
    let receipt = json_io::read(&directory.join("api.json"))?;
    json_io::exact(
        &receipt,
        &["format_version", "role", "scope_id", "identity"],
        "API 进程收据",
    )?;
    if receipt["format_version"] != 1 || receipt["role"] != "api" || receipt["scope_id"] != scope {
        return Err("API 进程收据角色或 scope 不匹配".into());
    }
    let identity = common::validate_process_identity(&receipt["identity"])?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("API PID 无效")?)
        .map_err(|_| "API PID 超出范围")?;
    if common::process_identity(pid)?.as_ref() != Some(&identity) {
        return Err("API 内核进程身份不匹配".into());
    }
    if identity["executable"] != runtime["artifacts"]["api"]["path"] {
        return Err("API 内核进程与运行收据产物不匹配".into());
    }
    listener(json_io::text(&root["api_url"], "API 地址")?, pid)?;
    Ok(json!({"runtime":runtime,"api_process":identity}))
}

fn contract(backend: &std::path::Path, directory: &std::path::Path) -> Result<Value> {
    let runtime = json_io::read(&directory.join("runtime.json"))?;
    json_io::exact(
        &runtime,
        &[
            "format_version",
            "backend_root",
            "scope_id",
            "api_config_dir",
            "worker_config_dir",
            "api_configuration_sha256",
            "worker_configuration_sha256",
            "configuration_sha256",
            "worker_ready_url",
            "artifacts",
        ],
        "运行收据",
    )?;
    if runtime["format_version"] != 1 || runtime["backend_root"] != json!(backend) {
        return Err("运行收据版本或后端目录不匹配".into());
    }
    let api = path(&runtime["api_config_dir"], "API 配置目录")?;
    let worker = path(&runtime["worker_config_dir"], "Worker 配置目录")?;
    if api.file_name().and_then(|v| v.to_str()) != Some("config")
        || worker.file_name().and_then(|v| v.to_str()) != Some("worker-config")
        || api.parent() != worker.parent()
    {
        return Err("运行收据必须绑定同一受管配置根".into());
    }
    common::local_directory(backend, &api, "API 配置目录")?;
    common::local_directory(backend, &worker, "Worker 配置目录")?;
    let api_digest = configuration_digest(&api)?;
    let worker_digest = configuration_digest(&worker)?;
    if runtime["api_configuration_sha256"] != api_digest
        || runtime["worker_configuration_sha256"] != worker_digest
        || runtime["configuration_sha256"] != python_hash(&json!([api_digest, worker_digest]))?
    {
        return Err("运行收据与绑定配置摘要不匹配".into());
    }
    let mut api_table = crate::rate_limit::load_app_table(backend, Some(&api), "test")
        .map_err(|error| error.message().to_owned())?;
    let mut worker_table = crate::rate_limit::load_app_table(backend, Some(&worker), "test")
        .map_err(|error| error.message().to_owned())?;
    let api_id = api_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    let worker_id = worker_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    if api_id.is_none_or(|v| !(0..=1023).contains(&v))
        || worker_id.is_none_or(|v| !(0..=1023).contains(&v))
        || api_id == worker_id
        || api_table != worker_table
    {
        return Err("API 与 Worker 配置不匹配或实例编号重复".into());
    }
    if api_table.get("scope_id").and_then(toml::Value::as_str) != runtime["scope_id"].as_str() {
        return Err("运行 scope 与绑定配置不匹配".into());
    }
    let jobs = api_table
        .get("jobs")
        .and_then(toml::Value::as_table)
        .ok_or("运行配置缺少 jobs")?;
    let port = jobs
        .get("health_port")
        .and_then(toml::Value::as_integer)
        .filter(|value| (1..=65535).contains(value))
        .ok_or("jobs.health_port 无效")?;
    if jobs.get("mode").and_then(toml::Value::as_str) != Some("external")
        || jobs.get("health_host").and_then(toml::Value::as_str) != Some("127.0.0.1")
        || runtime["worker_ready_url"] != format!("http://127.0.0.1:{port}/readyz")
    {
        return Err("Worker 就绪端点与绑定配置不匹配".into());
    }
    verify_artifacts(directory, &runtime)?;
    Ok(runtime)
}

fn configuration_digest(directory: &std::path::Path) -> Result<String> {
    let mut files = serde_json::Map::new();
    for name in ["app.toml", "app.test.toml"] {
        let file = directory.join(name);
        if name == "app.toml" && !file.exists() {
            continue;
        }
        files.insert(name.into(), common::binding(&file)?["sha256"].clone());
    }
    python_hash(
        &json!({"environment":{"APP_ENV":"test"},"config_directory":directory,"files":files}),
    )
}

fn verify_artifacts(directory: &std::path::Path, runtime: &Value) -> Result<()> {
    json_io::exact(&runtime["artifacts"], &["api", "worker"], "运行产物")?;
    let binaries = json_io::read(&directory.join("binaries.json"))?;
    json_io::exact(
        &binaries,
        &[
            "ryframe",
            "ryframe-worker",
            "ryframe-reset",
            "ryframe-migrate",
        ],
        "二进制清单",
    )?;
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let file = path(&binaries[name], "运行产物")?;
        let binding = common::binding(&file)?;
        if runtime["artifacts"][role] != json!({"path":binding["path"],"sha256":binding["sha256"]})
        {
            return Err("运行二进制与登记摘要不匹配".into());
        }
    }
    Ok(())
}

fn python_hash(value: &Value) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(hex::encode(Sha256::digest(python_json(value)?.as_bytes())))
}

fn python_json(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}: {}",
                    python_json(&json!(key))?,
                    python_json(value)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::String(_) => {
            let encoded = serde_json::to_string(value)?;
            let mut output = String::new();
            for character in encoded.chars() {
                if character.is_ascii() {
                    output.push(character);
                } else {
                    for unit in character.encode_utf16(&mut [0; 2]) {
                        use std::fmt::Write;
                        write!(output, "\\u{unit:04x}")?;
                    }
                }
            }
            output
        }
        value => serde_json::to_string(value)?,
    })
}

fn listener(value: &str, pid: u32) -> Result<()> {
    let url = url::Url::parse(value).map_err(|_| "API 地址无效")?;
    let host = url.host_str().ok_or("API 地址缺少主机")?;
    let port = url.port_or_known_default().ok_or("API 地址缺少端口")?;
    let ip: IpAddr = host.parse().map_err(|_| "API 地址必须使用明确 IP")?;
    TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_secs(2))
        .map_err(|_| "API 监听端口不可达")?;
    listener_owner(port, pid)?;
    Ok(())
}

#[cfg(windows)]
fn listener_owner(port: u16, pid: u32) -> Result<()> {
    let script = format!(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); @(Get-NetTCPConnection -State Listen -LocalPort {port} | Select-Object LocalAddress,OwningProcess) | ConvertTo-Json -Compress"
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if !output.status.success() {
        return Err("API 监听所有者无法核验".into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let rows = value.as_array().ok_or("API 监听观察必须是列表")?;
    if !rows.iter().any(|row| {
        row["OwningProcess"].as_u64() == Some(u64::from(pid))
            && matches!(
                row["LocalAddress"].as_str(),
                Some("127.0.0.1" | "0.0.0.0" | "::1" | "::")
            )
    }) {
        return Err("API 监听端口不属于登记进程".into());
    }
    Ok(())
}

#[cfg(unix)]
fn listener_owner(port: u16, pid: u32) -> Result<()> {
    let needle = format!("{port:04X}");
    let mut inodes = std::collections::HashSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for line in std::fs::read_to_string(table)?.lines().skip(1) {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() > 9
                && fields[1]
                    .rsplit_once(':')
                    .is_some_and(|(_, value)| value == needle)
                && fields[3] == "0A"
            {
                inodes.insert(fields[9].to_owned());
            }
        }
    }
    for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let target = match std::fs::read_link(entry?.path()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let value = target.to_string_lossy();
        if value
            .strip_prefix("socket:[")
            .and_then(|value| value.strip_suffix(']'))
            .is_some_and(|value| inodes.contains(value))
        {
            return Ok(());
        }
    }
    Err("API 监听端口不属于登记进程".into())
}

#[cfg(not(any(windows, unix)))]
fn listener_owner(_: u16, _: u32) -> Result<()> {
    Err("当前平台不支持监听所有者核验".into())
}
fn path(value: &Value, label: &str) -> Result<PathBuf> {
    let path = PathBuf::from(json_io::text(value, label)?);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    Ok(path)
}

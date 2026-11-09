use std::{
    net::{IpAddr, SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{
    json as json_io, resources::common,
    target_verify::input::request_configuration::python_configuration_hash,
};
use crate::{
    Result,
    recovery::fixture_control::{
        artifact::runtime::configuration_digest, services::lifecycle::readonly::process,
    },
};

/// 产品根只来自已经绑定的运行合同；证据目录仍由协调根单独核验。
pub(crate) fn product_root(coordinator: &std::path::Path, runtime: &Value) -> Result<PathBuf> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
    let root = PathBuf::from(json_io::text(&runtime["backend_root"], "运行产品根")?);
    validate_absolute_path(&root, LocalTestPathKind::ExistingDirectory)?;
    let canonical = crate::recovery::fixture_control::portable_path(&root.canonicalize()?);
    if canonical != root {
        return Err("运行产品根必须是规范普通目录".into());
    }
    if root != coordinator {
        common::local_directory(coordinator, &root, "发布运行产品根")?;
    }
    Ok(root)
}
/// 来源选择与已登记产品桥接必须独立指向同一根；不从构建收据补造字段。
pub(crate) fn selected_product_root(
    coordinator: &std::path::Path,
    target: &Value,
    scope: &super::super::bridge::scope::Scope,
    build_path: &std::path::Path,
    build: &Value,
) -> Result<PathBuf> {
    let (selected, _) = super::super::target_verify::input::request::execution_backend(
        coordinator,
        target.get("execution_backend"),
        &super::super::target_verify::input::request::SystemSourceObserver,
    )?;
    let registered = scope.registered_product_root(build_path, build)?;
    if selected != registered {
        return Err("目标产品根与本运行已核验构建桥接根不一致".into());
    }
    product_root(coordinator, &json!({"backend_root":selected}))
}
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
    let canonical = crate::recovery::fixture_control::portable_path(&backend.canonicalize()?);
    if canonical != backend {
        return Err("后端目录必须是规范绝对路径".into());
    }
    let directory = common::local_directory(&backend, &directory, "运行目录")?;
    let declared = json_io::read(&directory.join("runtime.json"))?;
    let product = product_root(&backend, &declared)?;
    let runtime = contract(&backend, &product, &directory)?;
    if runtime != declared {
        return Err("运行来源合同在核对期间发生变化".into());
    }
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
    listener(json_io::text(&root["api_url"], "API 地址")?, &identity)?;
    Ok(json!({"runtime":runtime,"api_process":identity}))
}

pub(crate) fn contract(
    coordinator: &std::path::Path,
    product: &std::path::Path,
    directory: &std::path::Path,
) -> Result<Value> {
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
    if runtime["format_version"] != 1 || runtime["backend_root"] != json!(product) {
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
    common::local_directory(coordinator, &api, "API 配置目录")?;
    common::local_directory(coordinator, &worker, "Worker 配置目录")?;
    let api_digest = configuration_digest(&api)?;
    let worker_digest = configuration_digest(&worker)?;
    if runtime["api_configuration_sha256"] != api_digest
        || runtime["worker_configuration_sha256"] != worker_digest
        || runtime["configuration_sha256"]
            != python_configuration_hash(&json!([api_digest, worker_digest]))?
    {
        return Err("运行收据与绑定配置摘要不匹配".into());
    }
    let mut api_table = crate::rate_limit::load_app_table(product, Some(&api), "test")
        .map_err(|error| error.message().to_owned())?;
    let mut worker_table = crate::rate_limit::load_app_table(product, Some(&worker), "test")
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

pub(crate) fn listener(value: &str, identity: &Value) -> Result<()> {
    let url = url::Url::parse(value).map_err(|_| "API 地址无效")?;
    let ip = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
        _ => return Err("API 地址必须使用明确 IP".into()),
    };
    let port = url.port_or_known_default().ok_or("API 地址缺少端口")?;
    process::require(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("API PID 无效")?)
        .map_err(|_| "API PID 超出范围")?;
    TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_secs(2))
        .map_err(|_| "API 监听端口不可达")?;
    // 连接探测与所有者观察共用该完整身份窗口，不把 PID 当作代次凭据。
    process::require(identity)?;
    listener_owner(port, pid)?;
    process::require(identity)
}

#[cfg(windows)]
fn listener_owner(port: u16, pid: u32) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        #[serde(rename = "LocalAddress")]
        address: String,
        #[serde(rename = "LocalPort")]
        port: u16,
        #[serde(rename = "OwningProcess")]
        owner: u32,
        #[serde(rename = "State")]
        state: u8,
    }
    let connection = wmi::WMIConnection::with_namespace_path("ROOT\\StandardCimv2")
        .map_err(|_| "API 监听 CIM 连接失败；检查本机读取权限，保留现场")?;
    let rows: Vec<Row> = connection.raw_query(format!(
        "SELECT LocalAddress,LocalPort,OwningProcess,State FROM MSFT_NetTCPConnection WHERE LocalPort={port} AND State=2"))
        .map_err(|_| "API 监听 CIM 查询失败；检查本机读取权限，保留现场")?;
    for row in &rows {
        if row.port != port || row.state != 2 || row.address.parse::<IpAddr>().is_err() {
            return Err("API 监听 CIM 返回不符合固定查询的记录".into());
        }
    }
    if !rows.iter().any(|row| {
        row.owner == pid && matches!(row.address.as_str(), "127.0.0.1" | "0.0.0.0" | "::1" | "::")
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

#[cfg(all(test, windows))]
#[test]
#[ignore = "需要本机 CIM 权限；仅观察本测试进程的四种回环/通配临时监听"]
fn current_process_listener_proof_preserves_ipv4_ipv6_and_creation_identity() {
    let identity = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    for (binding, destination) in [
        ("127.0.0.1", "127.0.0.1"),
        ("0.0.0.0", "127.0.0.1"),
        ("::1", "::1"),
        ("::", "::1"),
    ] {
        let ip: IpAddr = binding.parse().unwrap();
        let socket = std::net::TcpListener::bind(SocketAddr::new(ip, 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        let destination: IpAddr = destination.parse().unwrap();
        let url = format!("http://{}", SocketAddr::new(destination, port));
        listener(&url, &identity).unwrap();
        assert_eq!(
            listener_owner(port, 1).unwrap_err().to_string(),
            "API 监听端口不属于登记进程"
        );
        let mut reused = identity.clone();
        reused["started"] = json!("1");
        assert_eq!(
            listener(&url, &reused).unwrap_err().to_string(),
            "登记进程已退出或内核创建身份变化"
        );
        drop(socket);
        assert_eq!(
            listener_owner(port, std::process::id())
                .unwrap_err()
                .to_string(),
            "API 监听端口不属于登记进程"
        );
    }
}

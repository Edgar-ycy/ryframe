use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use crate::{
    Result,
    process::{ChildGroup, stop_child},
    recovery::{
        clone::native::resources::common,
        monitoring::{
            contract::{
                CONFIG_NAMES, RUNTIME_DIRECTORIES, VALIDATION_NAMES, artifact,
                file_descriptor as descriptor, loopback_port,
            },
            http, webhook,
        },
    },
};

pub(crate) fn create_runtime_directories(run: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
    for name in RUNTIME_DIRECTORIES {
        let path = run.join(name);
        fs::create_dir(&path).map_err(|error| format!("无法创建监控目录 {name}：{error}"))?;
        paths.insert(name.to_owned(), path);
    }
    let events = paths["evidence"].join("webhook-events.jsonl");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&events)?;
    paths.insert("events".into(), events);
    for (name, filename) in CONFIG_NAMES {
        paths.insert((*name).to_owned(), paths["configs"].join(filename));
    }
    for (name, filename) in VALIDATION_NAMES {
        paths.insert(format!("validate-{name}"), paths["configs"].join(filename));
    }
    Ok(paths)
}

pub(crate) fn write_configs(binding: &Value, paths: &BTreeMap<String, PathBuf>) -> Result<Value> {
    let rules = binding["rules"].as_object().ok_or("监控规则无效")?;
    let staging_root = Path::new(
        binding["staging"]["path"]
            .as_str()
            .ok_or("监控 staging 清单路径无效")?,
    )
    .parent()
    .ok_or("监控 staging 清单缺少目录")?;
    for (name, path_name, relative) in [
        ("alerts", "rules", "deploy/prometheus/ryframe-alerts.yml"),
        (
            "tests",
            "rule-tests",
            "deploy/prometheus/ryframe-alerts.test.yml",
        ),
    ] {
        let rule_receipt = rules.get(name).ok_or("监控规则缺少固定项")?;
        artifact(rule_receipt, "监控规则", None)?;
        let source = staging_root.join(relative);
        if descriptor(&source)?.get("sha256") != rule_receipt.get("sha256") {
            return Err("监控 staging 规则与绑定摘要不同".into());
        }
        fs::copy(&source, &paths[path_name])?;
        if descriptor(&paths[path_name])?.get("sha256") != rule_receipt.get("sha256") {
            return Err("监控规则在复制到配置目录时发生变化".into());
        }
    }
    for (name, content) in generated_configs(binding, paths)? {
        let mut stream = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&paths[&name])?;
        stream.write_all(content.as_bytes())?;
        stream.sync_all()?;
    }
    let mut receipts = Map::new();
    for (name, _) in CONFIG_NAMES {
        receipts.insert(name.to_owned(), descriptor(&paths[name])?);
    }
    Ok(Value::Object(receipts))
}

pub(crate) fn generated_configs(
    binding: &Value,
    paths: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, String>> {
    let product = binding["authority"]["endpoints"]
        .as_object()
        .ok_or("监控产品端点无效")?;
    let api = product["api_metrics"].as_str().ok_or("API 指标端点无效")?;
    let worker = product["worker_metrics"]
        .as_str()
        .ok_or("Worker 指标端点无效")?;
    let (_, api_authority, api_path) = http::loopback_url(api)?;
    let (_, worker_authority, worker_path) = http::loopback_url(worker)?;
    let (_, alert_authority, _) = http::loopback_url(
        binding["endpoints"]["alertmanager"]
            .as_str()
            .ok_or("Alertmanager 端点无效")?,
    )?;
    let quote = |value: &str| serde_json::to_string(value);
    let rule = path_text(&paths["rules"], "监控规则")?;
    let credential = binding["credential"]["path"]
        .as_str()
        .ok_or("监控凭据路径无效")?;
    let prometheus = format!(
        "global:\n  scrape_interval: 1s\n  evaluation_interval: 1s\nrule_files:\n  - {}\nalerting:\n  alertmanagers:\n    - static_configs:\n        - targets: [{}]\nscrape_configs:\n  - job_name: ryframe-api\n    metrics_path: {}\n    authorization:\n      type: Bearer\n      credentials_file: {}\n    static_configs:\n      - targets: [{}]\n  - job_name: ryframe-worker\n    metrics_path: {}\n    authorization:\n      type: Bearer\n      credentials_file: {}\n    static_configs:\n      - targets: [{}]\n",
        quote(&rule)?,
        quote(&alert_authority)?,
        quote(&api_path)?,
        quote(credential)?,
        quote(&api_authority)?,
        quote(&worker_path)?,
        quote(credential)?,
        quote(&worker_authority)?,
    );
    let webhook = format!(
        "{}/alerts",
        binding["endpoints"]["webhook"]
            .as_str()
            .ok_or("Webhook 端点无效")?
    );
    let alertmanager = format!(
        "global: {{}}\nroute:\n  receiver: ryframe-local-webhook\n  group_by: [alertname, probe_id]\n  group_wait: 0s\n  group_interval: 1s\n  repeat_interval: 1m\nreceivers:\n  - name: ryframe-local-webhook\n    webhook_configs:\n      - url: {}\n        send_resolved: true\n",
        quote(&webhook)?,
    );
    Ok(BTreeMap::from([
        ("prometheus".into(), prometheus),
        ("alertmanager".into(), alertmanager),
    ]))
}

pub(crate) fn validate_configs(
    binding: &Value,
    paths: &BTreeMap<String, PathBuf>,
    environment: &BTreeMap<String, String>,
) -> Result<Value> {
    let tools = binding["tools"].as_object().ok_or("监控工具无效")?;
    let tool = |name: &str| -> Result<String> {
        tools[name]["path"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("监控工具 {name} 路径无效").into())
    };
    let commands = [
        (
            "alertmanager",
            vec![
                tool("amtool")?,
                "check-config".into(),
                path_text(&paths["alertmanager"], "Alertmanager 配置")?,
            ],
        ),
        (
            "prometheus",
            vec![
                tool("promtool")?,
                "check".into(),
                "config".into(),
                path_text(&paths["prometheus"], "Prometheus 配置")?,
            ],
        ),
        (
            "rules",
            vec![
                tool("promtool")?,
                "check".into(),
                "rules".into(),
                path_text(&paths["rules"], "告警规则")?,
            ],
        ),
        (
            "boundaries",
            vec![
                tool("promtool")?,
                "test".into(),
                "rules".into(),
                path_text(&paths["rule-tests"], "告警规则测试")?,
            ],
        ),
    ];
    let mut receipts = Map::new();
    for (name, command) in commands {
        let log = &paths[&format!("validate-{name}")];
        run_logged(
            command,
            &paths["configs"],
            environment,
            log,
            Duration::from_secs(300),
        )?;
        receipts.insert(name.to_owned(), descriptor(log)?);
    }
    Ok(Value::Object(receipts))
}

pub(crate) fn role_commands(
    binding: &Value,
    paths: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<&'static str, Vec<String>>> {
    let port = |name: &str| {
        binding["ports"][name]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| format!("监控端口 {name} 无效"))
    };
    let tools = binding["tools"].as_object().ok_or("监控工具无效")?;
    let tool = |name: &str| -> Result<String> {
        Ok(tools[name]["path"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("监控工具 {name} 路径无效"))?)
    };
    let webhook = webhook::Protocol {
        host: "127.0.0.1".into(),
        port: port("webhook")?,
        events: paths["events"].clone(),
    };
    Ok(BTreeMap::from([
        (
            "webhook",
            vec![
                std::env::current_exe()?.to_string_lossy().into_owned(),
                "__monitoring-webhook".into(),
                webhook.encode()?,
            ],
        ),
        (
            "alertmanager",
            vec![
                tool("alertmanager")?,
                format!(
                    "--config.file={}",
                    path_text(&paths["alertmanager"], "Alertmanager 配置")?
                ),
                format!(
                    "--storage.path={}",
                    path_text(&paths["alertmanager-data"], "Alertmanager 数据目录")?
                ),
                format!("--web.listen-address=127.0.0.1:{}", port("alertmanager")?),
                "--cluster.listen-address=".into(),
            ],
        ),
        (
            "prometheus",
            vec![
                tool("prometheus")?,
                format!(
                    "--config.file={}",
                    path_text(&paths["prometheus"], "Prometheus 配置")?
                ),
                format!(
                    "--storage.tsdb.path={}",
                    path_text(&paths["prometheus-data"], "Prometheus 数据目录")?
                ),
                format!("--web.listen-address=127.0.0.1:{}", port("prometheus")?),
                "--storage.tsdb.retention.time=2h".into(),
            ],
        ),
    ]))
}

pub(crate) fn safe_environment(run: &Path) -> Result<BTreeMap<String, String>> {
    safe_environment_from(run, std::env::vars())
}

pub(crate) fn safe_environment_from(
    run: &Path,
    inherited: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>> {
    const SYSTEM: &[&str] = &[
        "APPDATA",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
        "COMMONPROGRAMW6432",
        "COMSPEC",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "LANG",
        "LOCALAPPDATA",
        "NUMBER_OF_PROCESSORS",
        "OS",
        "PATH",
        "PATHEXT",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "SHELL",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TMPDIR",
        "USER",
        "USERPROFILE",
        "WINDIR",
    ];
    let mut environment = inherited
        .into_iter()
        .filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            SYSTEM.contains(&key.as_str())
                || key.starts_with("PROCESSOR_")
                || key.starts_with("LC_")
        })
        .collect::<BTreeMap<_, _>>();
    environment.insert("NO_PROXY".into(), "127.0.0.1,localhost,::1".into());
    environment.insert("TEMP".into(), path_text(run, "监控运行目录")?);
    environment.insert("TMP".into(), path_text(run, "监控运行目录")?);
    Ok(environment)
}

pub(crate) fn run_logged(
    command: Vec<String>,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    log: &Path,
    timeout: Duration,
) -> Result<()> {
    let output = OpenOptions::new().write(true).create_new(true).open(log)?;
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output));
    let group = ChildGroup::new()?;
    let mut child = group.spawn(process)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            stop_child(&mut child)?;
            if status.success() {
                return Ok(());
            }
            return Err(format!("监控工具验证失败：{status}").into());
        }
        if Instant::now() >= deadline {
            let cleanup = stop_child(&mut child);
            return Err(format!("监控工具验证超时；回收状态：{cleanup:?}").into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn wait_ready(url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(response) = http::request(url, &[], None, Duration::from_secs(2))
            && response.status == 200
            && response.body.len() <= 64 * 1024
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("监控进程未在期限内就绪：{url}").into())
}

pub(crate) fn require_listening(pid: u64, url: &str) -> Result<()> {
    if pid <= 1 {
        return Err("监控产品 PID 无效".into());
    }
    if !common::port_ready(url, Duration::from_millis(500)) {
        return Err(format!("监控端点未监听：{url}").into());
    }
    Ok(())
}

pub(crate) fn verify_ports_closed(binding: &Value) -> Result<()> {
    for endpoint in binding["endpoints"]
        .as_object()
        .into_iter()
        .flat_map(|value| value.values())
    {
        let url = endpoint.as_str().ok_or("监控端点无效")?;
        let port = loopback_port(url, "")?;
        common::require_closed_port(url)
            .map_err(|error| format!("监控端口 {port} 未释放：{error}"))?;
    }
    Ok(())
}

pub(crate) fn write_bytes_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut stream = OpenOptions::new().write(true).create_new(true).open(path)?;
    stream.write_all(bytes)?;
    stream.sync_all()?;
    Ok(())
}

pub(crate) fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}

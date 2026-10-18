use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::MonitoringBindOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::{ChildGroup, stop_child},
    recovery::monitoring::staging,
};

pub(crate) const TOOL_VERSIONS: [(&str, &str); 4] = [
    ("prometheus", "prometheus, version 3.5.0"),
    ("promtool", "promtool, version 3.5.0"),
    ("alertmanager", "alertmanager, version 0.34.0"),
    ("amtool", "amtool, version 0.34.0"),
];

pub(crate) fn paths(
    options: &MonitoringBindOptions,
    backend: &Path,
) -> Result<BTreeMap<String, PathBuf>> {
    let values = [
        ("prometheus", &options.tools.prometheus),
        ("promtool", &options.tools.promtool),
        ("alertmanager", &options.tools.alertmanager),
        ("amtool", &options.tools.amtool),
    ];
    let mut result = BTreeMap::new();
    for (name, path) in values {
        validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("监控工具 {name} 必须位于协调器忽略目录：{error}"))?;
        result.insert(name.to_owned(), path.clone());
    }
    Ok(result)
}

pub(crate) fn versions(paths: &BTreeMap<String, PathBuf>) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for (name, prefix) in TOOL_VERSIONS {
        let path = paths.get(name).ok_or("监控工具集不完整")?;
        result.insert(name.to_owned(), tool_version(path, prefix)?);
    }
    Ok(result)
}

pub(crate) fn receipts(
    files: &BTreeMap<String, PathBuf>,
    versions: &BTreeMap<String, String>,
) -> Result<Value> {
    let mut result = Map::new();
    for (name, version) in versions {
        let path = files
            .get(&staging::tool_relative(name))
            .ok_or("监控 staging 缺少工具")?;
        let relative = staging::tool_relative(name);
        let mut receipt = staging::file_receipt(path, &relative)?;
        receipt["path"] = json!(path.to_string_lossy());
        receipt["version"] = json!(version);
        result.insert(name.clone(), receipt);
    }
    Ok(Value::Object(result))
}

fn tool_version(path: &Path, prefix: &str) -> Result<String> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .env_clear()
        .envs(super::config::safe_environment(
            path.parent().ok_or("监控工具缺少目录")?,
        )?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    let reader = child
        .take_stdout_reader()?
        .ok_or("监控工具版本命令没有输出")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let cleanup = stop_child(&mut child);
            return Err(format!("监控工具版本命令超时：{cleanup:?}").into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    stop_child(&mut child)?;
    let output = output
        .join()
        .map_err(|_| "监控工具版本输出读取线程失败")??;
    if !status.success() || output.len() > 16 * 1024 {
        return Err(format!("监控工具版本命令失败或输出过大：{status}").into());
    }
    let line = String::from_utf8(output)?
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if line.is_empty() || !line.is_ascii() || line.len() > 512 || !line.starts_with(prefix) {
        return Err("监控工具版本不是固定合同".into());
    }
    Ok(line)
}

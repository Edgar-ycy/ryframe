use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, stop_child},
    recovery::{
        clone::native::json as json_io,
        monitoring::contract::{ROLES, exact, process_identity, valid_operation_id},
    },
};

const CONTROL_POLL: Duration = Duration::from_millis(25);
const MAX_PROTOCOL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct Protocol {
    pub(crate) runtime_directory: PathBuf,
    pub(crate) role: String,
    pub(crate) scope_id: String,
    pub(crate) operation_id: String,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) log: PathBuf,
}

impl Protocol {
    pub(crate) fn encode(&self) -> Result<String> {
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<_, _>>();
        Ok(serde_json::to_string(&json!({
            "command": self.command,
            "cwd": self.cwd.to_string_lossy(),
            "environment": environment,
            "log": self.log.to_string_lossy(),
            "operation_id": self.operation_id,
            "role": self.role,
            "runtime_directory": self.runtime_directory.to_string_lossy(),
            "scope_id": self.scope_id,
        }))?)
    }

    pub(crate) fn decode(source: &str) -> Result<Self> {
        let value = json_io::parse_fragment(source, "监控 supervisor 私有协议")?;
        let object = exact(
            &value,
            &[
                "command",
                "cwd",
                "environment",
                "log",
                "operation_id",
                "role",
                "runtime_directory",
                "scope_id",
            ],
            "监控 supervisor 私有协议",
        )?;
        let command = array_text(object.get("command"), "supervisor 命令")?;
        if command.is_empty() || !Path::new(&command[0]).is_absolute() {
            return Err("监控 supervisor 命令必须使用绝对可执行文件".into());
        }
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .filter(|value| ROLES.contains(value))
            .ok_or("监控 supervisor 角色无效")?
            .to_owned();
        let operation_id = object
            .get("operation_id")
            .and_then(Value::as_str)
            .filter(|value| valid_operation_id(value))
            .ok_or("监控 supervisor 操作 ID 无效")?
            .to_owned();
        let scope_id = object
            .get("scope_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("监控 supervisor scope 无效")?
            .to_owned();
        let runtime_directory = absolute(object.get("runtime_directory"), "supervisor 运行目录")?;
        let cwd = absolute(object.get("cwd"), "supervisor 工作目录")?;
        let log = absolute(object.get("log"), "supervisor 日志")?;
        let environment = object
            .get("environment")
            .and_then(Value::as_object)
            .ok_or("监控 supervisor 环境必须为对象")?
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_owned()))
                    .ok_or_else(|| "监控 supervisor 环境值必须是字符串".into())
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(Self {
            runtime_directory,
            role,
            scope_id,
            operation_id,
            command,
            cwd,
            environment,
            log,
        })
    }
}

pub(crate) struct Invocation {
    pub(crate) executable: PathBuf,
    pub(crate) arguments: Vec<String>,
    pub(crate) stdin: Vec<u8>,
}

pub(crate) fn supervisor_invocation(protocol: &Protocol) -> Result<Invocation> {
    let executable = std::env::current_exe()?;
    supervisor_invocation_with_executable(protocol, &executable)
}

pub(crate) fn supervisor_invocation_with_executable(
    protocol: &Protocol,
    executable: &Path,
) -> Result<Invocation> {
    let mut stdin = protocol.encode()?.into_bytes();
    stdin.push(b'\n');
    Ok(Invocation {
        executable: executable.to_path_buf(),
        arguments: vec!["__monitoring-supervisor".into()],
        stdin,
    })
}

pub(crate) fn read_protocol_from_stdin() -> Result<Protocol> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_PROTOCOL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > MAX_PROTOCOL_BYTES {
        return Err("监控 supervisor 私有协议为空或超过 1 MiB".into());
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| "监控 supervisor 私有协议不是 UTF-8")?;
    Protocol::decode(source)
}

pub(crate) fn run(protocol: &Protocol) -> Result<()> {
    validate_protocol(protocol)?;
    let supervisor = crate::recovery::clone::native::lifecycle::identity::current()?;
    let ready = ready_path(protocol);
    let tree = tree_path(protocol);
    let result = result_path(protocol);
    let stopped = stopped_path(protocol);
    if ready.exists() || tree.exists() || result.exists() || stopped.exists() {
        return Err("监控 supervisor 已有同操作收据，拒绝接管".into());
    }
    let output = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&protocol.log)
        .map_err(|error| format!("无法打开监控进程日志：{error}"))?;
    let mut command = Command::new(&protocol.command[0]);
    command
        .args(&protocol.command[1..])
        .current_dir(&protocol.cwd)
        .env_clear()
        .envs(&protocol.environment)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(output.try_clone()?))
        .stderr(std::process::Stdio::from(output));
    let group = ChildGroup::new()?;
    let mut child = group.spawn(command)?;
    let product =
        match crate::recovery::clone::native::resources::common::process_identity(child.id()) {
            Ok(Some(value)) => value,
            Ok(None) => return Err("监控产品进程在登记前退出".into()),
            Err(error) => return Err(format!("无法核验监控产品进程身份：{error}").into()),
        };
    let process_value = process_identity(&product, "监控产品进程")?.1;
    let supervisor_value = process_identity(&supervisor, "监控 supervisor 进程")?.1;
    let group_id = child.id();
    register_members(protocol, &supervisor_value, &process_value, group_id)?;
    let outcome = supervise_loop(protocol, &mut child);
    complete(
        protocol,
        &mut child,
        &supervisor_value,
        &process_value,
        outcome,
    )
}

fn register_members(
    protocol: &Protocol,
    supervisor_value: &Value,
    process_value: &Value,
    group_id: u32,
) -> Result<()> {
    let ready = ready_path(protocol);
    let tree = tree_path(protocol);
    write_protocol_json(
        &ready,
        &json!({
            "format_version": 1,
            "kind": "restore-monitoring-members-ready",
            "directory": protocol.runtime_directory.to_string_lossy(),
            "operation_id": protocol.operation_id,
            "role": protocol.role,
            "scope_id": protocol.scope_id,
            "supervisor": supervisor_value,
            "process": process_value,
            "status": "joined",
        }),
    )?;
    write_protocol_json(
        &tree,
        &json!({
            "format_version": 3,
            "kind": "restore-monitoring-process-tree",
            "runtime_directory": protocol.runtime_directory.to_string_lossy(),
            "role": protocol.role,
            "scope_id": protocol.scope_id,
            "operation_id": protocol.operation_id,
            "supervisor": supervisor_value,
            "process": process_value,
            "group_id": group_id,
        }),
    )?;
    Ok(())
}

fn complete(
    protocol: &Protocol,
    child: &mut ManagedChild,
    supervisor_value: &Value,
    process_value: &Value,
    outcome: Result<Option<String>>,
) -> Result<()> {
    let result = result_path(protocol);
    let stopped = stopped_path(protocol);
    let (exit_code, termination, error_type) = match outcome {
        Ok(Some(mode)) => {
            stop_child(child)?;
            (
                0,
                if mode == "crash" { "forced" } else { "normal" },
                Value::Null,
            )
        }
        Ok(None) => {
            let status = child.try_wait()?.ok_or("监控产品进程状态未知")?;
            (status.code().unwrap_or(1), "natural", Value::Null)
        }
        Err(error) => {
            let cleanup = stop_child(child);
            (
                1,
                "forced",
                Value::String(if cleanup.is_ok() {
                    error.to_string()
                } else {
                    format!("{error}/{cleanup:?}")
                }),
            )
        }
    };
    write_protocol_json(
        &result,
        &json!({
            "format_version": 1,
            "kind": "restore-monitoring-process-tree-result",
            "operation_id": protocol.operation_id,
            "process": process_value,
            "exit_code": exit_code,
            "termination": termination,
        }),
    )?;
    write_protocol_json(
        &stopped,
        &json!({
            "format_version": 1,
            "kind": "restore-monitoring-members-stopped",
            "directory": protocol.runtime_directory.to_string_lossy(),
            "operation_id": protocol.operation_id,
            "role": protocol.role,
            "scope_id": protocol.scope_id,
            "supervisor": supervisor_value,
            "process": process_value,
            "members": [supervisor_value, process_value],
            "status": if error_type.is_null() { "stopped" } else { "failed" },
            "error_type": error_type,
        }),
    )?;
    Ok(())
}

fn supervise_loop(protocol: &Protocol, child: &mut ManagedChild) -> Result<Option<String>> {
    let control = control_path(protocol);
    let deadline = Instant::now() + Duration::from_secs(24 * 60 * 60);
    loop {
        if child.try_wait()?.is_some() {
            return Ok(None);
        }
        if control.exists() {
            let document = json_io::read(&control)?;
            let object = exact(
                &document,
                &[
                    "format_version",
                    "kind",
                    "operation_id",
                    "mode",
                    "requested_at_ns",
                ],
                "监控进程树停止请求",
            )?;
            let mode = object
                .get("mode")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "normal" | "crash"))
                .ok_or("监控进程树停止模式无效")?;
            if object.get("format_version").and_then(Value::as_u64) != Some(1)
                || object.get("kind").and_then(Value::as_str)
                    != Some("restore-monitoring-process-tree-control")
                || object.get("operation_id").and_then(Value::as_str)
                    != Some(protocol.operation_id.as_str())
            {
                return Err("监控进程树停止请求与当前操作不同".into());
            }
            return Ok(Some(mode.to_owned()));
        }
        if Instant::now() >= deadline {
            return Err("监控 supervisor 超过最长运行期限".into());
        }
        thread::sleep(CONTROL_POLL);
    }
}

pub(crate) fn request_stop(tree: &Value, crash: bool) -> Result<()> {
    let role = tree["role"].as_str().ok_or("监控进程树缺少角色")?;
    let operation = tree["operation_id"]
        .as_str()
        .ok_or("监控进程树缺少操作 ID")?;
    let directory = PathBuf::from(
        tree["runtime_directory"]
            .as_str()
            .ok_or("监控进程树缺少运行目录")?,
    );
    let path = directory.join(format!("{role}-tree-{operation}-control.json"));
    json_io::write_new(
        &path,
        &json!({
            "format_version": 1,
            "kind": "restore-monitoring-process-tree-control",
            "operation_id": operation,
            "mode": if crash { "crash" } else { "normal" },
            "requested_at_ns": UtcTimestamp::now(),
        }),
    )
}

pub(crate) fn wait_stopped(tree: &Value, timeout: Duration) -> Result<Value> {
    let role = tree["role"].as_str().ok_or("监控进程树缺少角色")?;
    let operation = tree["operation_id"]
        .as_str()
        .ok_or("监控进程树缺少操作 ID")?;
    let directory = PathBuf::from(
        tree["runtime_directory"]
            .as_str()
            .ok_or("监控进程树缺少运行目录")?,
    );
    let path = directory.join(format!("{role}-members-{operation}-stopped.json"));
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            let document = json_io::read(&path)?;
            validate_stopped(tree, &document)?;
            identity_stopped(
                tree.get("supervisor").ok_or("监控进程树缺少 supervisor")?,
                deadline,
                "监控 supervisor",
            )?;
            return Ok(document);
        }
        if Instant::now() >= deadline {
            return Err(format!("监控进程 {role} 未在期限内发布停止证明").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn identity_stopped(expected: &Value, deadline: Instant, label: &str) -> Result<()> {
    let (pid, value) = process_identity(expected, label)?;
    loop {
        match crate::recovery::clone::native::resources::common::process_identity(pid) {
            Ok(None) => return Ok(()),
            Ok(Some(current)) if current != value => {
                return Err(format!("{label} PID 已复用，拒绝认定关闭成功").into());
            }
            Ok(Some(_)) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(Some(_)) => return Err(format!("{label} 未在期限内退出").into()),
            Err(error)
                if transient_access_error(&error.to_string()) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn transient_access_error(message: &str) -> bool {
    message.contains("0x0005")
        || message.contains("拒绝访问")
        || message.contains("Access is denied")
}

fn validate_stopped(tree: &Value, value: &Value) -> Result<()> {
    let object = exact(
        value,
        &[
            "format_version",
            "kind",
            "directory",
            "operation_id",
            "role",
            "scope_id",
            "supervisor",
            "process",
            "members",
            "status",
            "error_type",
        ],
        "监控完整成员停止证明",
    )?;
    if object.get("format_version").and_then(Value::as_u64) != Some(1)
        || object.get("kind").and_then(Value::as_str) != Some("restore-monitoring-members-stopped")
        || object.get("operation_id") != tree.get("operation_id")
        || object.get("role") != tree.get("role")
        || object.get("scope_id") != tree.get("scope_id")
        || object.get("directory") != tree.get("runtime_directory")
        || object.get("supervisor") != tree.get("supervisor")
        || object.get("process") != tree.get("process")
        || object.get("status").and_then(Value::as_str) != Some("stopped")
        || !object.get("error_type").is_some_and(Value::is_null)
    {
        return Err("监控完整成员停止证明与进程树不同".into());
    }
    let members = object
        .get("members")
        .and_then(Value::as_array)
        .filter(|value| !value.is_empty())
        .ok_or("监控完整成员停止证明缺少成员")?;
    for member in members {
        process_identity(member, "监控进程树成员")?;
    }
    Ok(())
}

pub(crate) fn tree_path_for(directory: &Path, role: &str) -> PathBuf {
    directory.join(format!("{role}-tree.json"))
}

fn validate_protocol(protocol: &Protocol) -> Result<()> {
    if !protocol.runtime_directory.is_absolute()
        || !protocol.runtime_directory.is_dir()
        || !protocol.cwd.is_absolute()
        || !protocol.log.starts_with(&protocol.runtime_directory)
        || protocol
            .environment
            .contains_key("APP_MONITOR_METRICS_BEARER_TOKEN")
    {
        return Err("监控 supervisor 私有协议路径或环境越界".into());
    }
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        if protocol
            .environment
            .keys()
            .any(|key| key.eq_ignore_ascii_case(name))
        {
            return Err("监控 supervisor 环境不能继承代理".into());
        }
    }
    Ok(())
}

fn ready_path(protocol: &Protocol) -> PathBuf {
    protocol.runtime_directory.join(format!(
        "{}-members-{}-ready.json",
        protocol.role, protocol.operation_id
    ))
}

fn tree_path(protocol: &Protocol) -> PathBuf {
    tree_path_for(&protocol.runtime_directory, &protocol.role)
}

fn result_path(protocol: &Protocol) -> PathBuf {
    protocol.runtime_directory.join(format!(
        "{}-tree-{}-result.json",
        protocol.role, protocol.operation_id
    ))
}

fn stopped_path(protocol: &Protocol) -> PathBuf {
    protocol.runtime_directory.join(format!(
        "{}-members-{}-stopped.json",
        protocol.role, protocol.operation_id
    ))
}

fn control_path(protocol: &Protocol) -> PathBuf {
    protocol.runtime_directory.join(format!(
        "{}-tree-{}-control.json",
        protocol.role, protocol.operation_id
    ))
}

fn write_protocol_json(path: &Path, value: &Value) -> Result<()> {
    json_io::write_new(path, value)
}

fn absolute(value: Option<&Value>, label: &str) -> Result<PathBuf> {
    let text = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{label}必须是绝对路径"))?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    Ok(path)
}

fn array_text(value: Option<&Value>, label: &str) -> Result<Vec<String>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label}必须是数组"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{label}项必须是字符串").into())
        })
        .collect()
}

struct UtcTimestamp;

impl UtcTimestamp {
    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64
    }
}

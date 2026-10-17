use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{
    digest, json as json_io,
    lifecycle::{identity, manifest},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) fn scope_id(value: &str) -> Result<()> {
    if !Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?.is_match(value) {
        return Err("资源 scope_id 无效".into());
    }
    Ok(())
}

pub(crate) fn binding(path: &Path) -> Result<Value> {
    manifest::binding(path, &digest::file(path)?)
}

pub(crate) fn bound_file(backend: &Path, value: &Value, label: &str) -> Result<PathBuf> {
    manifest::bound_file(backend, value, label, true)
}

pub(crate) fn read_bound(backend: &Path, value: &Value, label: &str) -> Result<Value> {
    json_io::read(&bound_file(backend, value, label)?)
}

pub(crate) fn local_directory(backend: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("{label}无效：{error}").into())
}

pub(crate) fn common_file(backend: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("{label}无效：{error}").into())
}

pub(crate) fn directory_identity(backend: &Path, value: &Value) -> Result<PathBuf> {
    let object = json_io::exact(value, &["path", "device", "inode"], "资源目录身份")?;
    let path = PathBuf::from(
        object
            .get("path")
            .and_then(Value::as_str)
            .ok_or("资源目录路径必须为字符串")?,
    );
    let path = local_directory(backend, &path, "资源数据目录")?;
    let metadata = fs::symlink_metadata(&path)?;
    if digest::linked(&metadata) || !metadata.is_dir() {
        return Err("资源数据目录必须是真实目录".into());
    }
    let (device, inode) = device_inode_for(&path, &metadata)?;
    if Some(device) != object.get("device").and_then(Value::as_u64)
        || Some(inode) != object.get("inode").and_then(Value::as_u64)
    {
        return Err("资源数据目录的实际文件身份变化".into());
    }
    Ok(path)
}

pub(crate) fn environment(backend: &Path, value: &Value) -> Result<BTreeMap<String, String>> {
    let private = read_bound(backend, value, "资源环境")?;
    let object = json_io::exact(&private, &["environment"], "资源环境")?;
    let values = json_io::object(object.get("environment").unwrap(), "资源环境变量")?;
    let mut result = BTreeMap::new();
    for (key, value) in values {
        result.insert(
            key.clone(),
            value
                .as_str()
                .ok_or("资源环境变量值必须为字符串")?
                .to_owned(),
        );
    }
    Ok(result)
}

pub(crate) fn endpoint(value: &str) -> Result<(u16, String)> {
    let prefix = "http://127.0.0.1:";
    let port = value
        .strip_prefix(prefix)
        .ok_or("资源端点必须为本机 HTTP 根地址")?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("资源端点端口无效".into());
    }
    let port: u16 = port.parse().map_err(|_| "资源端点端口无效")?;
    if port == 0 {
        return Err("资源端点端口无效".into());
    }
    Ok((port, format!("127.0.0.1:{port}")))
}

pub(crate) fn require_closed_port(value: &str) -> Result<()> {
    let (port, _) = endpoint(value)?;
    match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(error) => Err(format!("资源端口 {port} 仍被占用或无法核验：{error}").into()),
    }
}

pub(crate) fn port_ready(value: &str, timeout: Duration) -> bool {
    let Ok((port, _)) = endpoint(value) else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        timeout,
    )
    .is_ok()
}

pub(crate) fn process_identity(pid: u32) -> Result<Option<Value>> {
    identity::process(pid)
}

pub(crate) fn validate_process_identity(value: &Value) -> Result<Value> {
    let object = json_io::exact(value, &["pid", "started", "executable"], "服务进程身份")?;
    let pid = object
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 1)
        .ok_or("服务进程 PID 无效")?;
    let started = object
        .get("started")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("服务进程创建代次无效")?;
    let executable = object
        .get("executable")
        .and_then(Value::as_str)
        .filter(|value| Path::new(value).is_absolute())
        .ok_or("服务进程可执行文件必须为绝对路径")?;
    Ok(json!({
        "pid": pid,
        "started": started,
        "executable": executable,
    }))
}

pub(crate) fn require_recorded_stopped(expected: &Value) -> Result<()> {
    let expected = validate_process_identity(expected)?;
    let pid = expected["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("服务进程 PID 超出范围")?;
    match process_identity(pid)? {
        Some(current) if current == expected => Err("登记服务进程仍在运行".into()),
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

pub(crate) fn write_new(path: &Path, value: &Value) -> Result<()> {
    json_io::write_new(path, value)
}

pub(crate) fn ensure_new_directory(backend: &Path, path: &Path, label: &str) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::NewDirectory)
        .map_err(|error| format!("{label}无效：{error}"))?;
    fs::create_dir(path).map_err(|error| format!("无法创建{label}：{error}").into())
}

pub(crate) fn ensure_directory(backend: &Path, path: &Path, label: &str) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::StateDirectory)
        .map_err(|error| format!("{label}无效：{error}"))?;
    if !path.exists() {
        fs::create_dir(path).map_err(|error| format!("无法创建{label}：{error}"))?;
    }
    Ok(())
}

pub(crate) fn path_text(path: &Path, label: &str) -> Result<String> {
    manifest::path_text(path, label)
}

pub(crate) fn terminate_tree(expected: &Value) -> Result<bool> {
    let expected = validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().unwrap()).map_err(|_| "服务 PID 超出范围")?;
    let Some(current) = process_identity(pid)? else {
        return Ok(false);
    };
    if current != expected {
        return Err("服务 PID 已被复用，拒绝操作其他内核代次".into());
    }
    terminate_platform(pid)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match process_identity(pid)? {
            None => return Ok(true),
            Some(current) if current != expected => return Ok(true),
            Some(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Some(_) => return Err("服务进程树终止后仍在运行".into()),
        }
    }
}

pub(crate) fn terminate_pid_tree(pid: u32) -> Result<()> {
    if pid <= 1 {
        return Err("服务 PID 无效".into());
    }
    terminate_platform(pid)
}

pub(crate) fn command_line(
    executable: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Command> {
    let mut command = Command::new(executable);
    command.args(args).current_dir(cwd).env_clear();
    for (key, value) in std::env::vars_os() {
        if let Some(key) = key.to_str()
            && environment.contains_key(key)
        {
            continue;
        }
        command.env(key, value);
    }
    command.envs(environment);
    Ok(command)
}

pub(crate) fn pipe_to_file(command: &mut Command, stdout: &Path, stderr: &Path) -> Result<()> {
    let out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stdout)
        .map_err(|error| format!("无法创建服务 stdout：{error}"))?;
    let err = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stderr)
        .map_err(|error| format!("无法创建服务 stderr：{error}"))?;
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::from(out));
    command.stderr(std::process::Stdio::from(err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Ok(())
}

#[cfg(windows)]
fn device_inode_for(path: &Path, _metadata: &fs::Metadata) -> Result<(u64, u64)> {
    use winsafe::{self as w, co, prelude::*};

    let text = path.to_str().ok_or("Windows 数据目录必须为 UTF-8")?;
    let (handle, _) = w::HFILE::CreateFile(
        text,
        co::GENERIC::default(),
        Some(co::FILE_SHARE::READ | co::FILE_SHARE::WRITE | co::FILE_SHARE::DELETE),
        None,
        co::DISPOSITION::OPEN_EXISTING,
        co::FILE_ATTRIBUTE::NORMAL,
        Some(co::FILE_FLAG::BACKUP_SEMANTICS),
        None,
        None,
    )?;
    let mut information = w::BY_HANDLE_FILE_INFORMATION::default();
    handle.GetFileInformationByHandle(&mut information)?;
    let inode =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((u64::from(information.dwVolumeSerialNumber), inode))
}

#[cfg(unix)]
fn device_inode_for(_path: &Path, metadata: &fs::Metadata) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(any(unix, windows)))]
fn device_inode_for(_path: &Path, _metadata: &fs::Metadata) -> Result<(u64, u64)> {
    Err("当前平台无法取得资源目录文件身份".into())
}

#[cfg(windows)]
fn terminate_platform(pid: u32) -> Result<()> {
    let system_root = std::env::var_os("SystemRoot").ok_or("Windows 缺少 SystemRoot")?;
    let taskkill = PathBuf::from(system_root)
        .join("System32")
        .join("taskkill.exe");
    let mut command = Command::new(taskkill);
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};
        let handle = HPROCESS::OpenProcess(
            co::PROCESS::TERMINATE | co::PROCESS::SYNCHRONIZE,
            false,
            pid,
        )?;
        handle.TerminateProcess(1)?;
        Ok(())
    }
}

#[cfg(unix)]
fn terminate_platform(pid: u32) -> Result<()> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    let pid = i32::try_from(pid).map_err(|_| "服务 PID 超出 POSIX 范围")?;
    match kill(Pid::from_raw(-pid), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("无法终止服务进程组：{error}").into()),
    }
}

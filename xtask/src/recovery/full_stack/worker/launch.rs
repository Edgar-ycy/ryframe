use crate::Result;
use serde_json::{Value, json};
use std::{
    io::{Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
};

pub(super) fn environment(command: &mut Command) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "SYSTEMDRIVE",
            "COMSPEC",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "LANG",
            "LC_ALL",
            "LOCALAPPDATA",
            "APPDATA",
        ]
        .contains(&name.as_str())
        {
            command.env(key, value);
        }
    }
    command.env("APP_ENV", "test");
}
pub(super) fn command(operation: &str, args: &Value, directory: &Path) -> Result<Command> {
    let worker = operation != "run-frontend"
        && operation != "role-control"
        && args.get("role").is_none_or(|role| role == "worker");
    let request = json!({"format_version":1,"kind":if worker {"ryframe-full-stack-worker"} else {"ryframe-full-stack-process"},"operation":operation,"arguments":args});
    let mut input = tempfile::tempfile_in(directory)?;
    input.write_all(&serde_json::to_vec(&request)?)?;
    input.seek(SeekFrom::Start(0))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(if worker {
            "__full-stack-worker"
        } else {
            "__full-stack-process"
        })
        .stdin(input)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    environment(&mut command);
    if matches!(operation, "monitor" | "supervise" | "detach")
        && let Some(environment) = super::environment::Environment::arguments(args, directory)?
    {
        environment.apply(&mut command)?;
    }
    Ok(command)
}
#[cfg(windows)]
pub(super) fn detached(_operation: &str, args: &Value, directory: &Path) -> Result<u32> {
    // 短命跳板只继承受限环境；真正持久的 monitor 显式禁止任何句柄继承。
    let mut command = command("detach", args, directory)?;
    let output = command.stdout(Stdio::piped()).output()?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() || response["ok"] != true {
        return Err("Worker持久监督器创建未确认".into());
    }
    let pid = response["result"]
        .as_u64()
        .ok_or("Worker monitor未返回创建PID")?;
    Ok(u32::try_from(pid)?)
}
#[cfg(windows)]
pub(super) fn detach_platform(args: &Value) -> Result<Value> {
    crate::recovery::clone::native::json::exact(
        args,
        &["backend", "directory", "operation_id", "role"],
        "Worker跳板参数",
    )?;
    if !super::documents::hex(&args["operation_id"], 32) {
        return Err("Worker跳板操作ID无效".into());
    }
    let role = super::role::Role::parse(&args["role"])?;
    let executable = std::env::current_exe()?;
    let input = serde_json::to_string(
        &json!({"format_version":1,"kind":if role == super::role::Role::Worker {"ryframe-full-stack-worker"} else {"ryframe-full-stack-process"},"operation":"monitor","arguments":args}),
    )?;
    let entry = if role == super::role::Role::Worker {
        "__full-stack-worker-detached"
    } else {
        "__full-stack-process-detached"
    };
    let directory = Path::new(args["directory"].as_str().ok_or("缺少运行目录")?);
    let environment = super::environment::Environment::arguments(args, directory)?;
    if let Some(environment) = &environment {
        environment.inherited()?;
    }
    let pid = crate::process::windows_detach::spawn_detached(&executable, &[entry, &input])?;
    Ok(json!(pid))
}
#[cfg(unix)]
pub(super) fn detached(operation: &str, args: &Value, directory: &Path) -> Result<u32> {
    use std::os::unix::process::CommandExt;
    let mut command = command(operation, args, directory)?;
    let environment = super::environment::Environment::arguments(args, directory)?;
    if let Some(environment) = &environment {
        environment.apply(&mut command)?;
    }
    command.process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
pub(super) fn request(
    role: super::role::Role,
    backend: &Path,
    directory: &Path,
    id: &str,
) -> Value {
    json!({"backend":backend,"directory":directory,"operation_id":id,"role":role.name()})
}

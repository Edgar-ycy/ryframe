//! 持久 monitor 不继承控制器句柄；Windows 沿用已验证的短命跳板。
use super::*;
use std::process::{Command, Stdio};

pub(super) fn environment(command: &mut Command, actor: &actor::Actor) -> Result<()> {
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
    for (key, value) in actor.private.as_object().ok_or("RustFS 私有环境无效")? {
        command.env(key, value.as_str().ok_or("私有环境必须为字符串")?);
    }
    Ok(())
}

pub(super) fn command(actor: &actor::Actor, operation: &str, encoded: &str) -> Result<Command> {
    let descriptor = document::parse_fragment(encoded, "RustFS actor 原始绑定")?;
    let bound = common::bound_file(&actor.backend, &descriptor, "RustFS actor 原始绑定")?;
    check(
        bound == actor.path("actor.json")
            && document::read(&bound)? == serde_json::to_value(actor)?,
        "RustFS actor 原始授权变化",
    )?;
    let mut command = crate::process::child_command(std::env::current_exe()?);
    command
        .args([actor::ENTRY, operation, encoded])
        .current_dir(&actor.backend)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    environment(&mut command, actor)?;
    Ok(command)
}

fn descriptor(actor: &actor::Actor) -> Result<String> {
    let original = document::read(&actor.path("actor-binding.json"))?;
    binding::verify_snapshot(&original)?;
    Ok(serde_json::to_string(&original["binding"])?)
}

pub(super) fn start(actor: &actor::Actor, guard: &mut impl FnMut() -> Result<()>) -> Result<Value> {
    document::write_new(&actor.path("actor.json"), &serde_json::to_value(actor)?)?;
    document::write_new(
        &actor.path("actor-binding.json"),
        &binding::snapshot(&actor.path("actor.json"))?,
    )?;
    let encoded = descriptor(actor)?;
    #[cfg(windows)]
    let mut command = command(actor, "detach", &encoded)?;
    #[cfg(target_os = "linux")]
    let mut command = command(actor, "monitor", &encoded)?;
    // 与原监督入口相同：发起子进程前开始登记窗口，监听窗口另行开始。
    let deadline = Instant::now() + Duration::from_secs(actor.timeout()?);
    check(Instant::now() < deadline, "RustFS 监督登记期限耗尽")?;
    #[cfg(windows)]
    {
        let output = command.stdout(Stdio::piped()).output()?;
        check(Instant::now() < deadline, "RustFS 监督登记期限耗尽")?;
        check(
            output.status.success() && output.stdout == b"started\n",
            "RustFS monitor 跳板未确认",
        )?;
    }
    #[cfg(target_os = "linux")]
    {
        let mut child = command.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    return Err("当前平台不能保持 RustFS 完整监督语义".into());
    wait_registered(actor, deadline, guard)
}

fn wait_registered(
    actor: &actor::Actor,
    deadline: Instant,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<Value> {
    loop {
        check(Instant::now() < deadline, "RustFS 监督登记期限耗尽")?;
        actor.verify_bound()?;
        guard()?;
        check(Instant::now() < deadline, "RustFS 监督登记期限耗尽")?;
        check(
            !actor.path("actor-failed.json").try_exists()?,
            "RustFS 监督启动已失败",
        )?;
        if actor.path("rustfs-tree.json").try_exists()? && actor.path("rustfs.json").try_exists()? {
            let tree = actor::tree(actor)?;
            actor::running_tree(actor, &tree)?;
            check(Instant::now() < deadline, "RustFS 监督登记期限耗尽")?;
            return Ok(tree);
        }
        check(
            Instant::now() < deadline,
            "RustFS 未在期限内登记原完整监督树",
        )?;
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
pub(super) fn detach(_actor: &actor::Actor, encoded: &str) -> Result<()> {
    let executable = std::env::current_exe()?;
    crate::process::spawn_detached(&executable, &[actor::ENTRY, "monitor", encoded])?;
    use std::io::Write;
    std::io::stdout().lock().write_all(b"started\n")?;
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn detach(_actor: &actor::Actor, _encoded: &str) -> Result<()> {
    Err("RustFS 跳板仅供 Windows 固定 monitor".into())
}

//! Linux 实际输入、代次与收据核验；不读取发行版环境变量。
use super::super::diagnostic::{Category, Outcome, failure};
use super::super::{
    super::{digest, json as doc, resources::common},
    model,
};
use super::{Launch, owned, publish};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
};

pub(super) fn boot() -> Result<String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if !model::boot(&json!(value))? {
        return Err("缓存 Linux boot 无效".into());
    }
    Ok(value)
}

pub(super) fn identity(pid: u32) -> Result<Option<Value>> {
    let before = boot()?;
    let path = format!("/proc/{pid}");
    let stat = match fs::read_to_string(format!("{path}/stat")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let fields = stat
        .rsplit_once(')')
        .ok_or("缓存 stat 无名称边界")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.first() == Some(&"Z") {
        return Ok(None);
    }
    let started = *fields.get(19).ok_or("缓存 stat 无创建时间")?;
    let executable = fs::read_link(format!("{path}/exe"))?;
    let after = fs::read_to_string(format!("{path}/stat"))?;
    if after
        .rsplit_once(')')
        .ok_or("缓存 stat 无名称边界")?
        .1
        .split_whitespace()
        .nth(19)
        != Some(started)
        || boot()? != before
    {
        return Err("缓存身份观察窗口变化".into());
    }
    Ok(Some(
        json!({"pid":pid,"started":started,"executable":executable,"boot_id":before}),
    ))
}

pub(super) fn bind_and_close(port: u16) -> Result<()> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(false)?;
    socket.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
    drop(socket);
    Ok(())
}

pub(super) fn port(launch: &Launch) -> Result<u16> {
    let port = launch.request["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p >= 1024)
        .ok_or("缓存端口无效")?;
    Ok(port)
}

pub(super) fn validate(launch: &Launch) -> Result<()> {
    validate_inputs(launch).map_err(|error| {
        if error
            .downcast_ref::<super::super::diagnostic::Diagnostic>()
            .is_some()
        {
            error
        } else {
            failure(Category::ConfigurationDrift, Outcome::Unknown)
        }
    })
}

fn validate_inputs(launch: &Launch) -> Result<()> {
    launch.native_tool.verify_linux()?;
    model::plain(&launch.linux_output)?;
    let request = &launch.request;
    let config = model::linux_path(doc::text(&request["configuration"]["path"], "Redis 配置")?)?;
    let directory = model::linux_path(doc::text(&request["directory"]["path"], "Redis 目录")?)?;
    model::plain(Path::new(&config))?;
    model::plain(Path::new(&directory))?;
    let configuration = common::binding(Path::new(&config))?;
    if configuration["sha256"] != request["configuration"]["sha256"]
        || configuration["bytes"] != request["configuration"]["bytes"]
        || fs::canonicalize(doc::text(&request["launcher"], "Redis 入口")?)?
            != Path::new(doc::text(&request["executable"], "Redis 映像")?)
        || digest::file(&model::plain(Path::new(doc::text(
            &request["executable"],
            "Redis 映像",
        )?))?)?
        .sha256
            != doc::text(&request["sha256"], "Redis 摘要")?
    {
        return Err("缓存实际配置或 Redis 映像漂移".into());
    }
    let expected = model::configuration(request)?;
    let mut entries = serde_json::Map::new();
    let text = fs::read_to_string(&config)?;
    for line in text.lines() {
        let words = super::super::binding::request::words(line)?;
        if let Some(key) = words.first().filter(|key| expected.get(*key).is_some())
            && entries
                .insert(key.clone(), json!(words[1..].join(" ")))
                .is_some()
        {
            return Err("缓存固定配置重复".into());
        }
    }
    if Value::Object(entries) != expected || common::binding(Path::new(&config))? != configuration {
        return Err("缓存缺少非后台、固定目录和临时持久化配置".into());
    }
    Ok(())
}

pub(super) fn exec(launch: &Launch, request_path: &Path, sha: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;
    validate(launch).map_err(|error| {
        if error
            .downcast_ref::<super::super::diagnostic::Diagnostic>()
            .is_some()
        {
            error
        } else {
            failure(Category::ConfigurationDrift, Outcome::NotStarted)
        }
    })?;
    authorized_parent(launch, request_path, sha)
        .map_err(|_| failure(Category::ParentAuthorizationRejected, Outcome::NotStarted))?;
    nix::unistd::setsid().map_err(|error| {
        Box::new(super::super::diagnostic::Diagnostic {
            category: Category::ExecFailed,
            write_outcome: Outcome::NotStarted,
            os_code: Some(error as i32),
        }) as Box<dyn std::error::Error>
    })?;
    let config = model::linux_path(doc::text(
        &launch.request["configuration"]["path"],
        "Redis 配置",
    )?)?;
    let error = std::process::Command::new(doc::text(&launch.request["launcher"], "Redis 入口")?)
        .arg(config)
        .exec();
    Err(Box::new(super::super::diagnostic::Diagnostic {
        category: Category::ExecFailed,
        write_outcome: Outcome::NotStarted,
        os_code: error.raw_os_error(),
    }))
}

fn authorized_parent(launch: &Launch, request_path: &Path, sha: &str) -> Result<()> {
    let parent = nix::unistd::getppid().as_raw();
    let parent = u32::try_from(parent)?;
    let path = launch.linux_output.join("linux-intent.json");
    let before = common::binding(&path)?;
    let intent = doc::read(&path)?;
    let actual = identity(parent)?.ok_or("缓存 exec 没有存活父监督器")?;
    let args = fs::read(format!("/proc/{parent}/cmdline"))?;
    let argv = args
        .strip_suffix(&[0])
        .ok_or("缓存父监督器参数缺少终止边界")?
        .split(|byte| *byte == 0)
        .map(std::str::from_utf8)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let request_file = request_path.to_str().ok_or("缓存请求路径不是UTF-8")?;
    let expected_argv = [
        launch.native_tool.linux_program.as_str(),
        "__linux-cache",
        "serve",
        "--request",
        request_file,
        "--sha256",
        sha,
    ];
    let config = model::linux_path(doc::text(
        &launch.request["configuration"]["path"],
        "Redis 配置",
    )?)?;
    doc::exact(
        &intent,
        &["request_sha256", "arguments", "supervisor"],
        "缓存已授权监督意图",
    )?;
    if intent["request_sha256"] != launch.request_sha256
        || intent["arguments"] != json!([launch.request["launcher"], config])
        || intent["supervisor"] != actual
        || actual["executable"] != launch.native_tool.linux_program
        || argv != expected_argv
        || nix::unistd::getppid().as_raw() != i32::try_from(parent)?
        || identity(parent)?.as_ref() != Some(&actual)
        || common::binding(&path)? != before
    {
        return Err("缓存 exec 不是原 stdin 授权监督器的直接子进程".into());
    }
    Ok(())
}

pub(super) fn check(launch: &Launch, expected: &Value, stop: bool) -> Result<Value> {
    validate(launch)?;
    model::identity(expected, true)?;
    let receipt = doc::read(&launch.linux_output.join("linux-process.json"))?;
    let intent_path = launch.linux_output.join("linux-intent.json");
    let intent = doc::read(&intent_path)?;
    let config = model::linux_path(doc::text(
        &launch.request["configuration"]["path"],
        "Redis 配置",
    )?)?;
    if receipt
        != json!({"request_sha256":launch.request_sha256,"identity":expected,"intent":common::binding(&intent_path)?})
        || intent["request_sha256"] != launch.request_sha256
        || intent["arguments"] != json!([launch.request["launcher"], config])
    {
        return Err("缓存控制身份不属于原始创建收据".into());
    }
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("缓存 PID 无效")?)?;
    let before_boot = boot()?;
    let mut terminated = false;
    let mut alive = false;
    if expected["boot_id"] == before_boot {
        let fd = owned::open(pid)?;
        if let Some(fd) = fd {
            let before = identity(pid)?;
            if before.as_ref().is_some_and(|value| value != expected) {
                return Err(failure(Category::IdentityMismatch, Outcome::Unknown));
            }
            if before.is_some() && !owned::exited(&fd, std::time::Duration::ZERO)? {
                if identity(pid)?.as_ref() != Some(expected) || boot()? != before_boot {
                    return Err(failure(Category::IdentityMismatch, Outcome::Unknown));
                }
                if stop {
                    terminated = owned::stop(&fd)?;
                }
                alive = !owned::exited(&fd, std::time::Duration::ZERO)?;
                if alive && identity(pid)?.as_ref() != Some(expected) {
                    return Err(failure(Category::IdentityMismatch, Outcome::Unknown));
                }
            }
        }
    }
    if !alive {
        bind_and_close(port(launch)?)?;
    }
    if stop && alive {
        return Err("缓存精确停止未完成".into());
    }
    validate(launch)?;
    if boot()? != before_boot {
        return Err("缓存控制窗口 boot 变化".into());
    }
    Ok(json!({"identity":expected,"alive":alive,"boot_id":before_boot,"terminated":terminated}))
}

pub(super) fn receipt(launch: &Launch, name: &str, value: Value) -> Result<()> {
    publish(&launch.linux_output.join(name), &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_intent_still_rejects_an_independent_exec_parent_without_touching_receipts() {
        let root = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        let parent = u32::try_from(nix::unistd::getppid().as_raw()).unwrap();
        let actual = identity(parent).unwrap().unwrap();
        let mut request = super::super::super::native_lifecycle_tests::native_request(
            super::super::super::tests::request(),
        );
        request["native_tool"]["linux_program"] = actual["executable"].clone();
        let launch = Launch {
            format_version: 2,
            request_sha256: digest::plan_hash(&request).unwrap(),
            native_tool: serde_json::from_value(request["native_tool"].clone()).unwrap(),
            request,
            windows_output: "D:/fixture/output".into(),
            linux_output: directory.path().into(),
        };
        let intent = json!({"request_sha256":launch.request_sha256,
            "arguments":[launch.request["launcher"],model::linux_path(launch.request["configuration"]["path"].as_str().unwrap()).unwrap()],
            "supervisor":actual});
        receipt(&launch, "linux-intent.json", intent).unwrap();
        let before = fs::read(directory.path().join("linux-intent.json")).unwrap();
        assert!(
            authorized_parent(
                &launch,
                &directory.path().join("launch-request.json"),
                &"a".repeat(64)
            )
            .is_err()
        );
        assert_eq!(
            fs::read(directory.path().join("linux-intent.json")).unwrap(),
            before
        );
        assert!(!directory.path().join("linux-process.json").exists());
    }
}

//! 以独立跳板承接产品日志，并向原控制器交接已验证的进程身份。

use super::{EXEC_ENTRY, Launch, new_log, prepare_command, system_environment_key};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    process::windows_detach::spawn_detached_with_handle,
    recovery::clone::native::{
        digest, json as document, lifecycle::identity, target_verify::guard::Inputs,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use winsafe::{HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

const LAUNCH_TIMEOUT_MS: u32 = 15_000;
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    controller: Value,
    backend: PathBuf,
    artifact: PathBuf,
    configuration: PathBuf,
    environment: BTreeMap<String, String>,
    log: PathBuf,
}

struct Pending {
    path: PathBuf,
    request_sha256: String,
}

impl Pending {
    fn clear(&self) -> Result<()> {
        if digest::plan_hash(&read_file(&self.path)?)? != self.request_sha256 {
            return Err("本次角色启动待确认记录已变化，保留现场".into());
        }
        Ok(fs::remove_file(&self.path)?)
    }
}

pub(in super::super) struct OwnedChild {
    pid: u32,
    handle: CloseHandleGuard<HPROCESS>,
    pending: Option<Pending>,
}

impl OwnedChild {
    pub(in super::super) fn id(&self) -> u32 {
        self.pid
    }

    pub(in super::super) fn is_exited(&self) -> Result<bool> {
        exited(&self.handle)
    }

    pub(in super::super) fn kill(&self, registered: bool) -> Result<()> {
        Ok(self.handle.TerminateProcess(u32::from(!registered))?)
    }

    pub(in super::super) fn clear_pending(&mut self) -> Result<()> {
        if let Some(pending) = &self.pending {
            pending.clear()?;
            self.pending = None;
        }
        Ok(())
    }
}

fn pending_path(directory: &Path, role: &str) -> PathBuf {
    directory.join(format!("{role}.spawn-pending.json"))
}

pub(in super::super) fn check_pending(directory: &Path, role: &str) -> Result<()> {
    match fs::symlink_metadata(pending_path(directory, role)) {
        Ok(_) => Err("该角色仍有未确认的 Windows 启动，须核对原待确认记录及启动日志".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(in super::super) fn spawn(launch: &Launch<'_>) -> Result<OwnedChild> {
    let executable = std::env::current_exe()?;
    let request = Request {
        controller: identity::current()?,
        backend: launch.backend.to_path_buf(),
        artifact: launch.artifact.to_path_buf(),
        configuration: launch.configuration.to_path_buf(),
        environment: launch
            .environment
            .iter()
            .filter(|(name, _)| system_environment_key(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        log: launch.log.to_path_buf(),
    };
    request.verify_paths()?;
    let path = request.pending_path()?;
    let path_text = path.to_str().ok_or("启动待确认记录路径必须为 UTF-8")?;
    let encoded = serde_json::to_value(&request)?;
    let request_sha256 = digest::plan_hash(&encoded)?;
    validate_absolute_path(&path, LocalTestPathKind::OutputFile)?;
    document::write_new(&path, &encoded)?;
    let pending = Pending {
        path: path.clone(),
        request_sha256,
    };
    let launcher = match spawn_detached_with_handle(
        &executable,
        &[EXEC_ENTRY, path_text, &pending.request_sha256],
    ) {
        Ok(launcher) => launcher,
        Err(error) => {
            pending.clear()?;
            return Err(error);
        }
    };
    if launcher
        .hProcess
        .WaitForSingleObject(Some(LAUNCH_TIMEOUT_MS))?
        != co::WAIT::OBJECT_0
        || launcher.hProcess.GetExitCodeProcess()? != 0
    {
        return Err("Windows 角色跳板结果未确认，保留待确认记录、回传和日志".into());
    }
    let response = read_file(&request.report_path())?;
    let response = document::exact(
        &response,
        &["launcher_pid", "request_sha256", "identity"],
        "Windows 角色跳板回传",
    )?;
    if response["launcher_pid"] != json!(launcher.dwProcessId)
        || response["request_sha256"] != pending.request_sha256
    {
        return Err("Windows 角色跳板回传不属于本次启动，保留现场".into());
    }
    let product = &response["identity"];
    if !product.is_null()
        && let Some(mut child) = open_product(product, &request.artifact)?
    {
        child.pending = Some(pending);
        return Ok(child);
    }
    pending.clear()?;
    Err("Windows 角色未能启动或已退出，保留本次回传和日志".into())
}

fn exited(handle: &HPROCESS) -> Result<bool> {
    Ok(handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0)
}

fn open_product(expected: &Value, artifact: &Path) -> Result<Option<OwnedChild>> {
    let pid = process_id(expected)?;
    if process_image(expected)? != artifact {
        return Err("Windows 角色跳板回传映像与登记产物不同，保留现场".into());
    }
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE | co::PROCESS::TERMINATE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if exited(&handle)? {
        return Ok(None);
    }
    let actual = identity::windows_snapshot(&handle, pid);
    let wait = if actual.is_err() { 1000 } else { 0 };
    let Some(actual) = identity::confirm_live_observation(
        actual,
        handle
            .WaitForSingleObject(Some(wait))
            .map(|status| status == co::WAIT::OBJECT_0)
            .map_err(Into::into),
    )?
    else {
        return Ok(None);
    };
    if &actual != expected || process_image(&actual)? != artifact {
        return Err("Windows 产品进程创建身份或映像与本次回传不同，保留现场".into());
    }
    Ok(Some(OwnedChild {
        pid,
        handle,
        pending: None,
    }))
}

fn process_id(identity: &Value) -> Result<u32> {
    document::exact(
        identity,
        &["pid", "started", "executable"],
        "Windows 进程身份",
    )?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("Windows 进程 PID 无效")?)?;
    if pid <= 1
        || identity["started"]
            .as_str()
            .and_then(|started| started.parse::<u64>().ok())
            .is_none_or(|started| started == 0)
    {
        return Err("Windows 进程 PID 或创建时间无效".into());
    }
    Ok(pid)
}

fn process_image(identity: &Value) -> Result<&Path> {
    Ok(Path::new(document::text(
        &identity["executable"],
        "Windows 进程映像",
    )?))
}

fn read_file(path: &Path) -> Result<Value> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    document::read(path)
}

impl Request {
    fn pending_path(&self) -> Result<PathBuf> {
        let directory = self.log.parent().ok_or("角色日志缺少运行目录")?;
        let name = self
            .log
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("角色日志文件名必须为 UTF-8")?;
        let (role, token) = name
            .strip_suffix(".log")
            .and_then(|stem| stem.split_once('-'))
            .ok_or("角色日志必须绑定本次启动令牌")?;
        if !matches!(role, "api" | "worker")
            || token.len() != 32
            || !token
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("角色日志的角色或启动令牌无效".into());
        }
        Ok(pending_path(directory, role))
    }

    fn report_path(&self) -> PathBuf {
        self.log.with_extension("spawn-result.json")
    }

    fn verify_controller(&self) -> Result<()> {
        let pid = process_id(&self.controller)?;
        if identity::process(pid)?.as_ref() != Some(&self.controller) {
            return Err("Windows 角色启动待确认记录的原控制器已退出或变化".into());
        }
        Ok(())
    }

    fn verify_paths(&self) -> Result<()> {
        validate_absolute_path(&self.backend, LocalTestPathKind::ExistingDirectory)?;
        validate_absolute_path(&self.artifact, LocalTestPathKind::ExistingFile)?;
        validate_absolute_path(&self.configuration, LocalTestPathKind::ExistingDirectory)?;
        validate_absolute_path(&self.log, LocalTestPathKind::OutputFile)?;
        validate_absolute_path(&self.report_path(), LocalTestPathKind::OutputFile)?;
        Ok(())
    }

    fn report(&self, request_sha256: &str, product: Option<&Value>) -> Result<()> {
        self.verify_controller()?;
        document::write_new(
            &self.report_path(),
            &json!({"launcher_pid": std::process::id(), "request_sha256": request_sha256,
                "identity": product}),
        )
    }
}

pub(super) fn run_exec(args: &[String]) -> Result<()> {
    let [path, request_sha256] = args else {
        return Err("Windows 角色跳板需要唯一待确认记录及摘要".into());
    };
    let path = Path::new(path);
    let request_input = Inputs::open([path.to_path_buf()])?;
    let encoded = read_file(path)?;
    if digest::plan_hash(&encoded)? != *request_sha256 {
        return Err("Windows 角色跳板的待确认记录摘要不同".into());
    }
    let request: Request = serde_json::from_value(encoded)?;
    if request.pending_path()? != path
        || request
            .environment
            .keys()
            .any(|name| !system_environment_key(name))
    {
        return Err("Windows 角色跳板的记录路径或环境字段无效".into());
    }
    request.verify_controller()?;
    request.verify_paths()?;
    let artifact_input = Inputs::open([request.artifact.clone()])?;
    let log = new_log(&request.log)?;
    let mut command = Command::new(&request.artifact);
    command
        .arg("--config-dir")
        .arg(&request.configuration)
        .creation_flags(CREATE_NO_WINDOW);
    prepare_command(&mut command, &request.backend, &request.environment, &log)?;
    request_input.check()?;
    artifact_input.check()?;
    request.verify_controller()?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            writeln!(&log, "Windows 角色进程未创建：{error}")?;
            return request.report(request_sha256, None);
        }
    };
    handoff(&request, request_sha256, &mut child, &log)
}

fn handoff(
    request: &Request,
    request_sha256: &str,
    child: &mut Child,
    log: &fs::File,
) -> Result<()> {
    let product = match observe_launcher_child(child, &request.artifact) {
        Ok(product) => product,
        Err(error) => {
            if let Err(cleanup) = close_launcher_child(child) {
                return Err(format!("{error}；跳板自有角色退出未确认：{cleanup}").into());
            }
            writeln!(&*log, "Windows 角色进程未能完成身份交接：{error}")?;
            None
        }
    };
    if let Err(error) = request.report(request_sha256, product.as_ref()) {
        if let Err(cleanup) = close_launcher_child(child) {
            return Err(format!("{error}；跳板自有角色退出未确认：{cleanup}").into());
        }
        return Err(error);
    }
    Ok(())
}

fn observe_launcher_child(child: &mut Child, artifact: &Path) -> Result<Option<Value>> {
    let observed = identity::process(child.id());
    // 短命角色的内核查询可能与退出并发。以启动方持有的原 Child 确认退出。
    if child.try_wait()?.is_some() {
        return Ok(None);
    }
    let product = observed?.ok_or("Windows 角色创建身份未确认")?;
    if process_image(&product)? != artifact {
        return Err("Windows 角色进程映像与登记产物不同".into());
    }
    Ok(Some(product))
}

fn close_launcher_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if let Err(error) = child.kill()
        && child.try_wait()?.is_none()
    {
        return Err(error.into());
    }
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            return Err("Windows 跳板自有角色未在期限内退出".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

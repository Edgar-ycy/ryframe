//! 控制器仅接收本次原代次监督器的树，失败时按原操作收口。
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::{actor, state::State};
use crate::{
    Result,
    recovery::{
        clone::native::{json as strict, lifecycle::identity, resources::common},
        inputs::document,
        runtime::{
            REGISTRATION_LOCK,
            bind::ownership::Owner,
            dataset_authority::check,
            generation::{STATE, receipts},
            status::optional_file,
            stop,
        },
    },
};

const STARTUP: Duration = Duration::from_secs(5);
const CLEANUP: Duration = Duration::from_secs(10);

pub(super) struct Attempt {
    backend: PathBuf,
    directory: PathBuf,
    role: &'static str,
    operation: String,
    scope: Value,
    monitor_pid: Option<u32>,
    monitor: Option<Value>,
    sent: bool,
    #[cfg(target_os = "linux")]
    child: Option<std::process::Child>,
}

impl Attempt {
    pub(super) fn new(backend: &Path, state: &State, role: &'static str) -> Result<Self> {
        let generation = state.generation();
        Ok(Self {
            backend: backend.to_owned(),
            directory: state.directory().to_owned(),
            role,
            operation: document::text(&generation["roles"][role]["operation_id"], "恢复角色操作")?
                .to_owned(),
            scope: generation["request"]["authority"]["scope_id"].clone(),
            monitor_pid: None,
            monitor: None,
            sent: false,
            #[cfg(target_os = "linux")]
            child: None,
        })
    }

    pub(super) fn start(&mut self, state: &State, owner: &Owner) -> Result<Value> {
        let (request, log_identity) = self.descriptor(state, owner)?;
        let log = self.directory.join(format!("{}.log", self.role));
        let mut command = crate::process::child_command(std::env::current_exe()?);
        command
            .current_dir(&self.backend)
            .stdin(Stdio::null())
            .stderr(actor::open_log(&log, &log_identity)?);
        actor::restrict_environment(&mut command, false);
        let deadline = Instant::now() + STARTUP;
        self.spawn(command, &request, deadline)?;
        let pid = self.monitor_pid.ok_or("恢复启动缺少 monitor 进程编号")?;
        self.monitor = identity::process(pid)?;
        let monitor = self.monitor.as_ref().ok_or("恢复 monitor 在登记前已退出")?;
        loop {
            owner.check()?;
            check(Instant::now() < deadline, "恢复角色登记完整进程树超时")?;
            actor::require_identity(monitor)?;
            if let Some(tree) = self.tree()? {
                let process_path = self.directory.join(format!("{}.json", self.role));
                if optional_file(&process_path)? {
                    let (product, _) = receipts::process(&self.directory, self.role, &self.scope)?;
                    check(
                        self.ready()?.as_ref().is_some_and(|ready| {
                            ready["monitor"] == *monitor
                                && ready["supervisor"] == tree["supervisor"]
                        }) && tree["monitor"] == *monitor
                            && tree["process"] == product,
                        "恢复启动树未绑定原 monitor、supervisor 或产品",
                    )?;
                    for field in ["supervisor", "process", "monitor"] {
                        actor::require_identity(&tree[field])?;
                    }
                    owner.check()?;
                    check(Instant::now() < deadline, "恢复角色进程树确认超过启动期限")?;
                    return Ok(tree);
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn descriptor(&self, state: &State, owner: &Owner) -> Result<(String, Value)> {
        owner.check()?;
        let control = owner.control_directory()?;
        let owner_document = document::read(
            &control.join(REGISTRATION_LOCK).join("owner.json"),
            &self.backend,
            "原启动 owner",
        )?;
        let runtime = self.directory.parent().ok_or("恢复角色缺少运行根")?;
        let persisted = document::read(&runtime.join(STATE), &self.backend, "原启动代次状态")?;
        let active = strict::array(&persisted.value["generations"], "恢复启动代次")?.last();
        check(
            active == Some(state.generation()),
            "恢复启动持久代次发生变化",
        )?;
        let log = self.directory.join(format!("{}.log", self.role));
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&log)?
            .sync_all()?;
        let log_identity = actor::log_identity(&log)?;
        owner.check()?;
        persisted.assert_unchanged()?;
        owner_document.assert_unchanged()?;
        let request = serde_json::to_string(&json!({
            "backend":self.backend,"directory":self.directory,"role":self.role,
            "operation_id":self.operation,"state":document::binding(&persisted),
            "owner":document::binding(&owner_document),"log_identity":log_identity,
        }))?;
        Ok((request, log_identity))
    }

    #[cfg(windows)]
    fn spawn(&mut self, mut command: Command, request: &str, deadline: Instant) -> Result<()> {
        command
            .args([actor::ENTRY, "detach", request])
            .stdout(Stdio::piped());
        let mut bridge = command.spawn()?;
        self.sent = true;
        while bridge.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                bridge.kill()?;
                bridge.wait()?;
                return Err("恢复 monitor 跳板启动结果未知，保留原代次".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let output = bridge.wait_with_output()?;
        check(output.status.success(), "恢复 monitor 跳板未确认创建")?;
        self.monitor_pid = Some(std::str::from_utf8(&output.stdout)?.trim().parse()?);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn spawn(&mut self, mut command: Command, request: &str, _deadline: Instant) -> Result<()> {
        use std::os::unix::process::CommandExt;

        // 树外 monitor 脱离控制器的终端信号组，仍由原 PID 与收据确认。
        command
            .args([actor::ENTRY, "monitor", request])
            .process_group(0)
            .stdout(Stdio::null());
        let child = command.spawn()?;
        self.sent = true;
        self.monitor_pid = Some(child.id());
        self.child = Some(child);
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn spawn(&mut self, _: Command, _: &str, _: Instant) -> Result<()> {
        Err("当前平台不能保持恢复运行的完整进程树监督语义".into())
    }

    pub(super) fn cancel(&mut self, owner: &Owner) -> Result<Value> {
        let deadline = Instant::now() + CLEANUP;
        let ready = self.wait_ready_or_exit(owner, deadline)?;
        if let Some(ready) = ready {
            if self.monitor_alive(&ready["monitor"])? {
                if let Some(tree) = self.tree()? {
                    check(
                        !ready_missing_with_tree(Some(&ready), &tree),
                        "恢复启动失败树与原 monitor 不同",
                    )?;
                    stop::terminate_tree(&self.directory, self.role, &tree, || owner.check())?;
                } else {
                    self.request_stop(owner)?;
                }
            }
            self.wait_closed(&ready["monitor"], owner, deadline)?;
        }
        let tree = self.tree()?.unwrap_or(Value::Null);
        if ready_missing_with_tree(self.ready()?.as_ref(), &tree) {
            return Err("恢复启动失败现场缺少原 monitor 归属，保留原证据".into());
        }
        Ok(json!({"state":if tree.is_null() {"pending"} else {"stopped"},"tree":tree}))
    }

    fn wait_ready_or_exit(&mut self, owner: &Owner, deadline: Instant) -> Result<Option<Value>> {
        loop {
            owner.check()?;
            if let Some(ready) = self.ready()? {
                return Ok(Some(ready));
            }
            self.reap()?;
            if !self.sent
                || match self.monitor_pid {
                    Some(pid) => identity::process(pid)?.is_none(),
                    None => false,
                }
            {
                return self.ready();
            }
            check(Instant::now() < deadline, "恢复 monitor 启动或退出尚未明确")?;
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_closed(&mut self, monitor: &Value, owner: &Owner, deadline: Instant) -> Result<()> {
        while self.monitor_alive(monitor)? {
            owner.check()?;
            check(
                Instant::now() < deadline,
                "恢复 monitor 尚未确认退出，保留待定状态",
            )?;
            self.reap()?;
            thread::sleep(Duration::from_millis(20));
        }
        self.reap()?;
        owner.check()
    }

    fn monitor_alive(&self, monitor: &Value) -> Result<bool> {
        let pid = u32::try_from(document::integer(&monitor["pid"], "恢复 monitor PID")?)?;
        match identity::process(pid)? {
            Some(actual) if actual == *monitor => Ok(true),
            Some(_) => Err("恢复 monitor PID 已复用，保留原证据".into()),
            None => {
                stop::require_exited(monitor)?;
                Ok(false)
            }
        }
    }

    fn request_stop(&self, owner: &Owner) -> Result<()> {
        let path = actor::control_path(&self.directory, self.role, &self.operation);
        if crate::recovery::full_stack::worker::read_stop_request(&path, &json!(self.operation))?
            .is_none()
        {
            owner.check()?;
            let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
            strict::write_new(
                &path,
                &json!({"format_version":1,"kind":"full-stack-process-tree-control",
                "operation_id":self.operation,"mode":"normal","requested_at_ns":now}),
            )?;
        }
        Ok(())
    }

    fn ready(&self) -> Result<Option<Value>> {
        let path = self.directory.join(format!(
            "{}-members-{}-ready.json",
            self.role, self.operation
        ));
        if !optional_file(&path)? {
            return Ok(None);
        }
        let value = document::read(&path, &self.backend, "恢复角色原 monitor 归属")?.value;
        strict::exact(
            &value,
            &[
                "format_version",
                "operation_id",
                "directory",
                "role",
                "scope_id",
                "supervisor",
                "monitor",
            ],
            "恢复角色原 monitor 归属",
        )?;
        check(
            value["format_version"] == 1
                && value["operation_id"] == self.operation
                && value["directory"] == json!(self.directory)
                && value["role"] == self.role
                && value["scope_id"] == self.scope
                && self
                    .monitor_pid
                    .is_none_or(|pid| value["monitor"]["pid"] == pid)
                && self
                    .monitor
                    .as_ref()
                    .is_none_or(|monitor| value["monitor"] == *monitor),
            "恢复原 monitor 归属与本次角色操作不同",
        )?;
        common::validate_process_identity(&value["monitor"])?;
        common::validate_process_identity(&value["supervisor"])?;
        check(
            value["monitor"]["pid"] != value["supervisor"]["pid"],
            "恢复 monitor 与 supervisor 身份重叠",
        )?;
        Ok(Some(value))
    }

    fn tree(&self) -> Result<Option<Value>> {
        if !optional_file(&self.directory.join(format!("{}-tree.json", self.role)))? {
            return Ok(None);
        }
        let tree = receipts::tree(&self.directory, self.role, &self.scope)?;
        check(
            tree["operation_id"] == self.operation,
            "恢复角色原树操作编号不同",
        )?;
        Ok(Some(tree))
    }

    fn reap(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(child) = &mut self.child {
            child.try_wait()?;
        }
        Ok(())
    }

    pub(super) fn release(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(mut child) = self.child.take() {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn ready_missing_with_tree(ready: Option<&Value>, tree: &Value) -> bool {
    !tree.is_null()
        && ready.is_none_or(|ready| {
            tree["monitor"] != ready["monitor"] || tree["supervisor"] != ready["supervisor"]
        })
}

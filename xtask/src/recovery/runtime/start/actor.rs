//! 正式恢复的原生监督入口只消费当前启动 owner 和已持久化的角色意图。
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{json as strict, lifecycle::identity, resources::common},
        full_stack::worker::read_stop_request,
        inputs::document,
        runtime::{
            REGISTRATION_LOCK,
            dataset_authority::{check, commands, inputs::Inputs, path},
            generation::{self, ROLES, STATE},
            registration::backend_root,
        },
    },
};

#[path = "actor/monitor.rs"]
mod monitor;
#[path = "actor/supervisor.rs"]
mod supervisor;

pub(super) const ENTRY: &str = "__restore-runtime-actor";
const LIMIT: usize = 16 * 1024;

pub(super) struct Actor {
    pub(super) backend: PathBuf,
    pub(super) directory: PathBuf,
    pub(super) role: &'static str,
    pub(super) scope: Value,
    pub(super) operation_id: String,
    request: Value,
    selected: Value,
    encoded: String,
}

impl Actor {
    fn read(value: &Value, encoded: String) -> Result<Self> {
        strict::exact(
            value,
            &[
                "backend",
                "directory",
                "role",
                "operation_id",
                "state",
                "owner",
                "log_identity",
            ],
            "恢复启动监督描述",
        )?;
        let backend = backend_root(&path(&value["backend"])?)?;
        let directory = path(&value["directory"])?;
        validate_local_test_path(&directory, &backend, LocalTestPathKind::ExistingDirectory)?;
        let role = ROLES
            .into_iter()
            .find(|role| value["role"] == *role)
            .ok_or("恢复启动监督角色无效")?;
        let operation_id = document::text(&value["operation_id"], "恢复监督操作编号")?.to_owned();
        if !generation::hex(&value["operation_id"], 32) {
            return Err("恢复启动监督操作编号无效".into());
        }
        let (_, state) = document::bound_file(&backend, &value["state"], "已登记启动代次状态")?;
        let selected = current(&state.value)?;
        let mut inputs = Inputs::new(&backend);
        let request = &selected["request"];
        let commands = commands::verify_direct(request, &mut inputs)?;
        let runtime = directory.parent().ok_or("恢复监督代次目录缺少父目录")?;
        generation::request(&json!({
            "action":"validate-generation","root":runtime,"binding":request["registration"],
            "value":selected,"previous":null,"commands":commands,"number":selected["number"],
        }))?;
        let actor = Self {
            backend,
            directory,
            role,
            scope: request["authority"]["scope_id"].clone(),
            operation_id,
            request: request.clone(),
            selected: value.clone(),
            encoded,
        };
        actor.check_state(&state)?;
        inputs.finish(actor.verify_start())?;
        Ok(actor)
    }

    fn check_state(&self, state: &document::Document) -> Result<()> {
        let runtime = self.directory.parent().ok_or("恢复监督缺少运行根")?;
        let active = current(&state.value)?;
        check(
            state.path == runtime.join(STATE)
                && active["directory"] == json!(self.directory)
                && active["status"] == "starting"
                && active["roles"][self.role]["state"] == "pending"
                && active["roles"][self.role]["tree"].is_null()
                && active["roles"][self.role]["operation_id"] == self.operation_id
                && active["request"] == self.request,
            "恢复监督未绑定当前原始待启动代次与角色",
        )
    }

    pub(super) fn verify_start(&self) -> Result<()> {
        let (_, state) =
            document::bound_file(&self.backend, &self.selected["state"], "启动原代次状态")?;
        self.check_state(&state)?;
        let (_, owner) =
            document::bound_file(&self.backend, &self.selected["owner"], "原启动控制 owner")?;
        let control = self.backend.join(".local-tests");
        let binding = &self.request["registration"];
        let registration = document::digest(&binding["registration"]["sha256"], "启动登记摘要")?;
        let target = document::digest(&binding["target_plan"]["sha256"], "启动目标计划摘要")?;
        let expected = format!("runtime-start:{registration}:{target}");
        let value = &owner.value;
        strict::exact(
            value,
            &[
                "format_version",
                "kind",
                "identity",
                "runtime_directory",
                "operation",
                "token",
            ],
            "恢复监督启动 owner",
        )?;
        check(
            owner.path == control.join(REGISTRATION_LOCK).join("owner.json")
                && value["format_version"] == 1
                && value["kind"] == "restore-runtime-registration-controller"
                && value["runtime_directory"] == json!(control)
                && value["operation"] == expected
                && generation::hex(&value["token"], 32),
            "恢复监督启动 owner 不属于原登记",
        )?;
        require_identity(&value["identity"])?;
        state.assert_unchanged()?;
        owner.assert_unchanged()
    }

    pub(super) fn member_path(&self, suffix: &str) -> PathBuf {
        self.directory.join(format!(
            "{}-members-{}-{suffix}.json",
            self.role, self.operation_id
        ))
    }

    pub(super) fn control(&self) -> Result<Option<bool>> {
        read_stop_request(
            &control_path(&self.directory, self.role, &self.operation_id),
            &json!(self.operation_id),
        )
    }

    pub(super) fn command(&self, mode: &str) -> Result<Command> {
        let mut command = crate::process::child_command(std::env::current_exe()?);
        command
            .args([ENTRY, mode, &self.encoded])
            .current_dir(&self.backend)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(self.log()?);
        restrict_environment(&mut command, false);
        Ok(command)
    }

    fn log(&self) -> Result<File> {
        let path = self.directory.join(format!("{}.log", self.role));
        validate_local_test_path(&path, &self.backend, LocalTestPathKind::ExistingFile)?;
        open_log(&path, &self.selected["log_identity"])
    }
}

pub(super) fn control_path(directory: &Path, role: &str, operation_id: &str) -> PathBuf {
    directory.join(format!("{role}-tree-{operation_id}-control.json"))
}

pub(super) fn require_identity(value: &Value) -> Result<()> {
    common::validate_process_identity(value)?;
    let pid = u32::try_from(document::integer(&value["pid"], "恢复监督进程编号")?)?;
    check(
        identity::process(pid)?.as_ref() == Some(value),
        "恢复监督原进程已经退出或创建身份变化",
    )
}

fn current(state: &Value) -> Result<&Value> {
    strict::array(&state["generations"], "恢复启动代次")?
        .last()
        .ok_or_else(|| "恢复监督缺少当代启动意图".into())
}

pub(super) fn log_identity(path: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    check(
        metadata.is_file() && !crate::recovery::clone::native::digest::linked(&metadata),
        "恢复监督日志必须是原普通文件",
    )?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    Ok(json!([device, inode]))
}

pub(super) fn open_log(path: &Path, expected: &Value) -> Result<File> {
    check(
        log_identity(path)? == *expected,
        "恢复监督原日志身份发生变化",
    )?;
    let mut options = OpenOptions::new();
    options.append(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // 同一角色的产品与监督器可追加诊断，不能删除或替换日志。
        options.share_mode(3);
    }
    let file = options.open(path)?;
    check(
        same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)?
            && log_identity(path)? == *expected,
        "打开的恢复监督日志不是本次原文件",
    )?;
    Ok(file)
}

pub(in crate::recovery) fn restrict_environment(command: &mut Command, product_backend: bool) {
    command.env_clear().envs(system_environment());
    if product_backend {
        command.env("APP_ENV", "test");
    }
}

pub(in crate::recovery) fn system_environment()
-> impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)> {
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
        "TEMP",
        "TMP",
        "TMPDIR",
        "USER",
        "USERPROFILE",
        "WINDIR",
    ];
    std::env::vars_os().filter(|(key, _)| {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        SYSTEM.contains(&upper.as_str())
            || upper.starts_with("PROCESSOR_")
            || upper.starts_with("LC_")
    })
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next()?.to_str()? != ENTRY {
        return None;
    }
    let result = (|| -> Result<()> {
        let mode = arguments.next().ok_or("恢复监督缺少操作")?;
        let raw = arguments.next().ok_or("恢复监督缺少已登记描述")?;
        let raw = raw.into_string().map_err(|_| "恢复监督描述必须为 UTF-8")?;
        if arguments.next().is_some() || raw.len() > LIMIT {
            return Err("恢复监督描述超限或包含额外参数".into());
        }
        #[cfg(target_os = "linux")]
        if mode == "supervise" {
            nix::unistd::setsid()?;
        }
        let actor = Actor::read(&strict::parse_fragment(&raw, "恢复监督描述")?, raw)?;
        let outcome = match mode.to_str() {
            Some("monitor") => monitor::run(&actor),
            Some("supervise") => supervisor::run(&actor),
            #[cfg(windows)]
            Some("detach") => {
                let pid = crate::process::windows_detach::spawn_detached(
                    &std::env::current_exe()?,
                    &[ENTRY, "monitor", &actor.encoded],
                )?;
                println!("{pid}");
                Ok(())
            }
            _ => Err("恢复监督操作无效".into()),
        };
        match outcome {
            Err(error) if cfg!(windows) && mode == "monitor" => {
                match actor
                    .log()
                    .and_then(|mut log| writeln!(log, "恢复监督失败：{error}").map_err(Into::into))
                {
                    Ok(()) => Err(error),
                    Err(record) => Err(format!("{error}；原角色日志写入失败：{record}").into()),
                }
            }
            other => other,
        }
    })();
    if let Err(error) = result {
        eprintln!("恢复监督失败：{error}");
        Some(1)
    } else {
        Some(0)
    }
}

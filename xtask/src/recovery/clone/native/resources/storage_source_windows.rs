use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use winsafe::{co, prelude::kernel_Hprocess};

use super::{
    super::{
        json as document,
        lifecycle::{identity, lock::ProcessGuard},
        target_verify::guard::Inputs,
    },
    common,
};
use crate::{
    Result,
    cli::{CloneSide, StorageOperation, StorageOptions},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::windows_detach::spawn_detached_with_handle,
    workspace,
};

const TICKET: &str = "ticket.json";
const RESPONSE: &str = "result.json";

struct Controller {
    side: CloneSide,
    entry: &'static str,
    launches: &'static str,
    guard: &'static str,
    stage: &'static str,
    label: &'static str,
}

const SOURCE: Controller = Controller {
    side: CloneSide::Source,
    entry: "__clone-storage-source-exec",
    launches: "storage-source-launches",
    guard: "storage-source-launch.guard",
    stage: "storage-source",
    label: "来源",
};

const TARGET: Controller = Controller {
    side: CloneSide::Target,
    entry: "__clone-storage-target-exec",
    launches: "storage-target-launches",
    guard: "storage-target-launch.guard",
    stage: "storage-target",
    label: "目标",
};

impl Controller {
    fn description(&self, suffix: &str) -> String {
        format!("{}存储控制器{suffix}", self.label)
    }

    fn unknown(&self) -> String {
        format!(
            "Windows {}",
            self.description("结果未知，保留本次票据和原阶段账本，先核对现场")
        )
    }
}

pub(super) fn run(backend: &Path, options: &StorageOptions) -> Result<Value> {
    let controller = match options.side {
        CloneSide::Source => &SOURCE,
        CloneSide::Target => &TARGET,
    };
    if options.operation != StorageOperation::Restart {
        return Err(format!(
            "Windows {}存储控制器仅处理{}重启",
            controller.label, controller.label
        )
        .into());
    }
    let run_dir = common::local_directory(backend, &options.run_dir, "复制运行目录")?;
    if let Some(request) = &options.request {
        common::common_file(backend, request, "存储重启请求")?;
    }
    let parent = identity::current()?;
    let executable = std::env::current_exe()?;
    let guard = ProcessGuard::acquire_named(&run_dir, controller.guard)?;
    let launches = run_dir.join(controller.launches);
    common::ensure_directory(backend, &launches, &controller.description("票据根目录"))?;
    let directory = launches.join(uuid::Uuid::new_v4().simple().to_string());
    common::ensure_new_directory(backend, &directory, &controller.description("票据目录"))?;
    let held_directory = Inputs::directory(&directory)?;
    let path = directory.join(TICKET);
    write_new(
        backend,
        &path,
        &json!({"parent": parent, "run_dir": run_dir, "request": options.request}),
    )?;
    let held_ticket = Inputs::open([path.clone()])?;
    let ticket = common::binding(&path)?;
    let sha = ticket["sha256"].as_str().ok_or("存储控制器票据摘要无效")?;
    let path = common::path_text(&path, &controller.description("票据"))?;
    guard.check_named(&run_dir, controller.guard)?;

    let launcher = spawn_detached_with_handle(&executable, &[controller.entry, &path, sha])
        .map_err(|_| controller.unknown())?;
    let pid = launcher.dwProcessId;
    // 此目录的创建是原子授权信号。只有 WinSafe 句柄对应的控制器会继续阶段。
    common::ensure_new_directory(backend, &controller_path(&directory, pid), "控制器启动确认")
        .map_err(|_| controller.unknown())?;
    let response = (|| {
        if launcher.hProcess.WaitForSingleObject(None)? != co::WAIT::OBJECT_0
            || launcher.hProcess.GetExitCodeProcess()? != 0
        {
            return Err(controller.unknown().into());
        }
        guard.check_named(&run_dir, controller.guard)?;
        held_directory.check()?;
        held_ticket.check()?;
        let path = common::common_file(backend, &directory.join(RESPONSE), "控制器回传")?;
        let pending = Inputs::open([path.clone()])?;
        let response = document::read(&path)?;
        pending.check()?;
        confirmed_result(controller, response, &ticket, pid)
    })()
    .map_err(|_| controller.unknown())?;
    response.ok_or_else(|| {
        format!(
            "Windows {}存储重启阶段失败，请核对原阶段账本和受管日志",
            controller.label
        )
        .into()
    })
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    let entry = args.next()?;
    let controller = [&SOURCE, &TARGET]
        .into_iter()
        .find(|controller| entry.as_os_str() == controller.entry)?;
    Some(match (args.next(), args.next(), args.next()) {
        (Some(path), Some(sha), None) => {
            let result = sha
                .to_str()
                .ok_or("控制器票据摘要必须为 UTF-8")
                .map_err(Into::into)
                .and_then(|sha| execute(controller, &PathBuf::from(path), sha));
            i32::from(result.is_err())
        }
        _ => 1,
    })
}

fn execute(controller: &Controller, path: &Path, sha: &str) -> Result<()> {
    let backend = workspace::root_dir();
    let label = controller.description("票据");
    let path = common::common_file(&backend, path, &label)?;
    let held_ticket = Inputs::open([path.clone()])?;
    let ticket = common::binding(&path)?;
    if ticket["sha256"].as_str() != Some(sha) {
        return Err(controller.description("票据摘要不一致").into());
    }
    let payload = common::read_bound(&backend, &ticket, &label)?;
    document::exact(&payload, &["parent", "run_dir", "request"], &label)?;
    let run_dir = common::local_directory(
        &backend,
        &required_path(&payload["run_dir"])?,
        "复制运行目录",
    )?;
    let directory = ticket_directory(controller, &path, &run_dir)?;
    let held_directory = Inputs::directory(directory)?;
    let request = match &payload["request"] {
        Value::Null => None,
        value => Some(common::common_file(
            &backend,
            &required_path(value)?,
            "存储重启请求",
        )?),
    };
    let pid = std::process::id();
    authorize(controller, &backend, directory, &payload["parent"], pid)?;
    let result = super::run_storage_stage(
        &backend,
        &StorageOptions {
            run_dir,
            side: controller.side,
            operation: StorageOperation::Restart,
            request,
        },
    );
    held_directory.check()?;
    held_ticket.check()?;
    write_new(
        &backend,
        &directory.join(RESPONSE),
        &json!({"ticket": ticket, "pid": pid, "result": result.ok()}),
    )
}

fn required_path(value: &Value) -> Result<PathBuf> {
    Ok(PathBuf::from(value.as_str().ok_or("控制器票据路径无效")?))
}

fn ticket_directory<'a>(
    controller: &Controller,
    path: &'a Path,
    run_dir: &Path,
) -> Result<&'a Path> {
    let directory = path
        .parent()
        .ok_or_else(|| controller.description("票据缺少目录"))?;
    let name = directory.file_name().and_then(|name| name.to_str());
    if path.file_name().and_then(|name| name.to_str()) != Some(TICKET)
        || directory.parent() != Some(run_dir.join(controller.launches).as_path())
        || name.is_none_or(|name| {
            name.len() != 32
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    {
        return Err(controller.description("票据不属于固定运行目录").into());
    }
    Ok(directory)
}

fn controller_path(directory: &Path, pid: u32) -> PathBuf {
    directory.join(format!("controller-{pid}"))
}

fn authorize(
    controller: &Controller,
    backend: &Path,
    directory: &Path,
    parent: &Value,
    pid: u32,
) -> Result<()> {
    let parent = common::validate_process_identity(parent)?;
    let parent_pid = parent["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or("控制器父进程 PID 无效")?;
    if parent_pid == pid || parent["executable"] != identity::current()?["executable"] {
        return Err(controller.description("与父进程程序身份不匹配").into());
    }
    let marker = controller_path(directory, pid);
    loop {
        let marker_exists = match fs::symlink_metadata(&marker) {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if common::process_identity(parent_pid)?.as_ref() != Some(&parent) {
            return Err(controller.description("父进程已失联或身份变化").into());
        }
        if marker_exists {
            common::local_directory(backend, &marker, "控制器启动确认")?;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn confirmed_result(
    controller: &Controller,
    mut response: Value,
    ticket: &Value,
    pid: u32,
) -> Result<Option<Value>> {
    document::exact(&response, &["ticket", "pid", "result"], "控制器回传")?;
    if &response["ticket"] != ticket || response["pid"].as_u64() != Some(u64::from(pid)) {
        return Err(controller.description("回传不属于本次启动").into());
    }
    let result = response["result"].take();
    match result {
        Value::Null => Ok(None),
        Value::Object(_)
            if result["status"] == "stage_finished"
                && result["stage"] == controller.stage
                && result["mode"] == "restart"
                && result["attempt"].as_u64().is_some_and(|number| number > 0)
                && result["result"]["status"] == "storage_restarted"
                && result["result"]["side"] == controller.side.as_str()
                && result["result"]["attempt"] == result["attempt"] =>
        {
            Ok(Some(result))
        }
        _ => Err(controller.description("回传的阶段结果无效").into()),
    }
}

fn write_new(backend: &Path, path: &Path, value: &Value) -> Result<()> {
    validate_local_test_path(path, backend, LocalTestPathKind::OutputFile)?;
    document::write_new(path, value)
}

#[cfg(test)]
fn restart_result(controller: &Controller) -> Value {
    json!({
        "status": "stage_finished",
        "stage": controller.stage,
        "mode": "restart",
        "attempt": 4,
        "result": {"status": "storage_restarted", "side": controller.side.as_str(), "attempt": 4},
    })
}

#[cfg(test)]
#[test]
fn detached_source_return_requires_the_ticket_process_and_exact_stage() {
    let ticket = json!({"path": "ticket", "bytes": 200, "sha256": "test"});
    let result = restart_result(&SOURCE);
    let response = json!({"ticket": ticket, "pid": 17, "result": result});
    assert_eq!(
        confirmed_result(&SOURCE, response.clone(), &ticket, 17).unwrap(),
        Some(result)
    );
    assert!(confirmed_result(&SOURCE, response.clone(), &ticket, 18).is_err());
    assert!(confirmed_result(&SOURCE, response.clone(), &Value::Null, 17).is_err());
    let mut changed = response;
    changed["result"]["result"]["attempt"] = json!(5);
    assert!(confirmed_result(&SOURCE, changed, &ticket, 17).is_err());
    assert_eq!(
        confirmed_result(
            &SOURCE,
            json!({"ticket": ticket, "pid": 17, "result": null}),
            &ticket,
            17
        )
        .unwrap(),
        None
    );
}

#[cfg(test)]
#[test]
fn detached_restart_rejects_tickets_and_results_from_the_other_side() {
    let run = Path::new("fixture-run");
    let path = |launches| run.join(launches).join("a".repeat(32)).join(TICKET);
    for (controller, other) in [(&SOURCE, &TARGET), (&TARGET, &SOURCE)] {
        let own = path(controller.launches);
        let foreign = path(other.launches);
        assert_eq!(
            ticket_directory(controller, &own, run).unwrap(),
            own.parent().unwrap()
        );
        assert!(ticket_directory(controller, &foreign, run).is_err());

        let ticket = json!({"path": own, "bytes": 200, "sha256": "test"});
        let foreign_ticket = json!({"path": foreign, "bytes": 200, "sha256": "test"});
        let own_result = restart_result(controller);
        let response = json!({"ticket": ticket, "pid": 17, "result": own_result});
        assert_eq!(
            confirmed_result(controller, response.clone(), &ticket, 17).unwrap(),
            Some(own_result)
        );
        let mut wrong_ticket = response.clone();
        wrong_ticket["ticket"] = foreign_ticket;
        assert!(confirmed_result(controller, wrong_ticket, &ticket, 17).is_err());
        let mut wrong_stage = response.clone();
        wrong_stage["result"]["stage"] = json!(other.stage);
        assert!(confirmed_result(controller, wrong_stage, &ticket, 17).is_err());
        let mut wrong_side = response;
        wrong_side["result"]["result"]["side"] = json!(other.side.as_str());
        assert!(confirmed_result(controller, wrong_side, &ticket, 17).is_err());
    }
}

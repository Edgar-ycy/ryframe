//! 核对完整登记的 Windows Redis 运行、正常关闭及外部终止证据。
use super::{
    BoundLaunch, LAUNCH, ProtectedInputs, bound_path, common, digest, document, exact_arguments,
    failure, model, process, same_path,
};
use crate::{
    Result,
    recovery::clone::native::{
        cache::{facts, host_control},
        target_verify::authority,
    },
};
use serde_json::{Value, json};
use std::{io::ErrorKind, path::Path};

#[path = "observe/handoff.rs"]
pub(super) mod handoff;
#[path = "observe/incident.rs"]
pub(super) mod incident;

const RUNTIME_FIELDS: &[&str] = &[
    "format_version",
    "request_sha256",
    "output",
    "launch",
    "supervisor",
    "process",
    "process_receipt",
    "facts",
    "redis",
];
const RECEIPTS: &[&str] = &[
    "runtime.json",
    "authorized.json",
    "process.json",
    "acknowledged.json",
    "accepted.json",
];

pub(in crate::recovery::clone::native::cache) fn inspect(
    request: &Value,
    output: &Path,
) -> Result<Value> {
    inspect_generation(request, output, Generation::Current)
}

pub(in crate::recovery::clone::native::cache) fn inspect_reboot(
    request: &Value,
    output: &Path,
) -> Result<Value> {
    inspect_generation(request, output, Generation::SourceReboot)
}

fn inspect_generation(request: &Value, output: &Path, generation: Generation) -> Result<Value> {
    let backend = crate::workspace::root_dir();
    common::local_directory(&backend, output, "Windows Redis 运行目录")?;
    model::shape(request)?;
    if !entry_exists(&output.join("intent.json"))? {
        return before_intent(request, output);
    }
    if entry_exists(&output.join("failed.json"))? || entry_exists(&output.join("cancel.json"))? {
        return failure_runtime(request, output, false);
    }
    let runtime = document::read(&output.join("runtime.json"))?;
    let registered = Registered::open_registered(&backend, Some(request), &runtime, generation)?;
    if registered.launch.output != output {
        return Err("Windows Redis 运行收据不属于原观察目录".into());
    }
    registered.finish(Ok(runtime.clone()))
}

pub(in crate::recovery::clone::native::cache) fn predecessor_request(
    backend: &Path,
    runtime_path: &Path,
    runtime: &Value,
) -> Result<Value> {
    predecessor_generation(backend, runtime_path, runtime, Generation::Current)
}

pub(in crate::recovery::clone::native::cache) fn predecessor_reboot(
    backend: &Path,
    runtime_path: &Path,
    runtime: &Value,
) -> Result<Value> {
    predecessor_generation(backend, runtime_path, runtime, Generation::SourceReboot)
}

fn predecessor_generation(
    backend: &Path,
    runtime_path: &Path,
    runtime: &Value,
    generation: Generation,
) -> Result<Value> {
    if runtime_path != bound_path(runtime, "output")?.join("runtime.json") {
        return Err("Windows Redis 前驱收据不属于登记的运行目录".into());
    }
    let registered = Registered::open_registered(backend, None, runtime, generation)?;
    registered.finish(Ok(registered.launch.request.clone()))
}

fn before_intent(request: &Value, output: &Path) -> Result<Value> {
    let files = directory_files(output)?;
    let has_request = files.iter().any(|name| name == "request.json");
    let has_launch = files.iter().any(|name| name == LAUNCH);
    if files
        .iter()
        .any(|name| !["request.json", LAUNCH].contains(&name.as_str()))
        || has_launch && !has_request
    {
        return Err("Windows Redis 无启动意图的目录含未知写入或缺少原请求".into());
    }
    let protected = ProtectedInputs::open(files.iter().map(|name| output.join(name)))?;
    protected.finish((|| {
        if has_request && document::read(&output.join("request.json"))? != *request {
            return Err("Windows Redis 无启动意图的原请求变化".into());
        }
        if has_launch {
            let path = output.join(LAUNCH);
            let launch = BoundLaunch::load_frozen(&path, &digest::file(&path)?.sha256)?;
            if launch.request != *request
                || !same_path(
                    document::text(&launch.owner["executable"], "原控制器映像")?,
                    document::text(&launch.coordinator["path"], "原协调工具")?,
                )
            {
                return Err("Windows Redis 无启动意图的 launch 不属于原请求或协调工具".into());
            }
            model::file_bound(&launch.coordinator)?;
        }
        if directory_files(output)? != files {
            return Err("Windows Redis 无启动意图的目录在观察期间变化".into());
        }
        Ok(Value::Null)
    })())
}

fn failure_runtime(request: &Value, output: &Path, live: bool) -> Result<Value> {
    let proof = failure::read(request, output, live)?;
    Ok(json!({"state":"failed_cleaned","output":output,
        "request_sha256":digest::plan_hash(request)?,"windows_failure":proof["windows_closed"]}))
}

pub(super) fn check_failed(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    live: bool,
) -> Result<()> {
    let output = bound_path(runtime, "output")?;
    common::local_directory(backend, &output, "Windows Redis 失败运行目录")?;
    if failure_runtime(request, &output, live)? != *runtime {
        return Err("Windows Redis 失败运行投影与原关闭证明不同".into());
    }
    Ok(())
}

pub(in crate::recovery::clone::native::cache) fn observe(
    backend: &Path,
    request: &Value,
    runtime: &Value,
) -> Result<Value> {
    let registered = Registered::open(backend, Some(request), runtime)?;
    registered.finish(registered.observe_running())
}

pub(in crate::recovery::clone::native::cache) fn status(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    effective: Option<&Value>,
) -> Result<Value> {
    if effective.is_some_and(|current| current != request) {
        return Err("Windows Redis 状态拒绝不同的有效请求".into());
    }
    if runtime["state"] == "failed_cleaned" {
        check_failed(backend, request, runtime, true)?;
        return Ok(json!({"state":"stopped","launcher_alive":false,
            "windows":{"identity":runtime["windows_failure"]["process"],"alive":false}}));
    }
    let registered = Registered::open(backend, Some(request), runtime)?;
    registered.finish(registered.status())
}

pub(in crate::recovery::clone::native::cache) fn external_termination(
    backend: &Path,
    request: &Value,
    runtime: &Value,
) -> Result<Value> {
    let registered = Registered::open(backend, Some(request), runtime)?;
    registered.finish(registered.external_termination())
}

pub(in crate::recovery::clone::native::cache) fn reboot_exit(
    backend: &Path,
    request: &Value,
    runtime: &Value,
) -> Result<Value> {
    let registered =
        Registered::open_registered(backend, Some(request), runtime, Generation::SourceReboot)?;
    let observed = if registered.exited {
        (|| {
            if present(&runtime["supervisor"])? || present(&runtime["process"])? {
                return Err("Windows Redis 重启观察发现原进程仍在运行".into());
            }
            registered.check_stopped()?;
            Ok(json!({"state":"stopped","launcher_alive":false,
                "windows":{"identity":runtime["process"],"alive":false}}))
        })()
    } else {
        registered.external_termination()
    };
    registered.finish(observed)
}

struct Registered<'a> {
    launch: BoundLaunch,
    runtime: &'a Value,
    launch_inputs: ProtectedInputs,
    receipt_inputs: ProtectedInputs,
    exited: bool,
    incident_failure: Option<Value>,
}

enum Generation {
    Current,
    SourceReboot,
    IncidentReboot,
}

impl<'a> Registered<'a> {
    fn open(backend: &Path, request: Option<&Value>, runtime: &'a Value) -> Result<Self> {
        Self::open_registered(backend, request, runtime, Generation::Current)
    }

    fn open_registered(
        backend: &Path,
        request: Option<&Value>,
        runtime: &'a Value,
        generation: Generation,
    ) -> Result<Self> {
        document::exact(runtime, RUNTIME_FIELDS, "Windows Redis 运行收据")?;
        if runtime["format_version"] != 3 {
            return Err("Windows Redis 运行收据未绑定原请求".into());
        }
        let output = bound_path(runtime, "output")?;
        common::local_directory(backend, &output, "Windows Redis 运行目录")?;
        let sha = document::text(&runtime["launch"]["sha256"], "Windows Redis 启动摘要")?;
        let path = output.join(LAUNCH);
        let launch = match generation {
            Generation::Current => BoundLaunch::load(&path, sha)?,
            Generation::SourceReboot | Generation::IncidentReboot => {
                BoundLaunch::load_reboot(&path, sha)?
            }
        };
        if launch.output != output
            || std::fs::canonicalize(&launch.backend)? != std::fs::canonicalize(backend)?
            || request.is_some_and(|expected| launch.request != *expected)
            || runtime["request_sha256"] != digest::plan_hash(&launch.request)?
            || launch.binding != runtime["launch"]
        {
            return Err("Windows Redis 运行收据与冻结启动请求不同".into());
        }
        let launch_inputs = launch.protect()?;
        let exited = entry_exists(&output.join("exited.json"))?;
        let mut paths = RECEIPTS
            .iter()
            .map(|name| output.join(name))
            .collect::<Vec<_>>();
        if exited {
            paths.push(output.join("exited.json"));
        }
        let incident_failure = if matches!(generation, Generation::IncidentReboot) {
            let failure = output.join("failed.json");
            common::common_file(backend, &failure, "Windows Redis 原事故失败收据")?;
            let binding = common::binding(&failure)?;
            paths.push(failure);
            Some(binding)
        } else {
            None
        };
        let registered = Self {
            launch,
            runtime,
            launch_inputs,
            receipt_inputs: ProtectedInputs::open(paths)?,
            exited,
            incident_failure,
        };
        registered.check_evidence()?;
        Ok(registered)
    }

    fn check_evidence(&self) -> Result<()> {
        self.check_terminal()?;
        let runtime = self.runtime;
        let output = &self.launch.output;
        let supervisor = &runtime["supervisor"];
        let identity = &runtime["process"];
        common::validate_process_identity(supervisor)?;
        common::validate_process_identity(identity)?;
        let request = &self.launch.request;
        let tool = &request["tool"];
        let executable = document::text(&tool["path"], "Windows Redis 工具路径")?;
        let coordinator =
            document::text(&self.launch.coordinator["path"], "Windows Redis 监督器映像")?;
        if identity["pid"] == supervisor["pid"]
            || !same_path(
                document::text(&identity["executable"], "Windows Redis 映像")?,
                executable,
            )
            || !same_path(
                document::text(&supervisor["executable"], "监督器映像")?,
                coordinator,
            )
        {
            return Err("Windows Redis 监督器或进程身份未绑定冻结映像".into());
        }
        let receipt = &runtime["process_receipt"];
        let process_file = output.join("process.json");
        let authorization = json!({"launch":self.launch.binding,"supervisor":supervisor});
        if document::read(&output.join("runtime.json"))? != *runtime
            || document::read(&output.join("authorized.json"))? != authorization
            || common::binding(&process_file)? != *receipt
            || document::read(&process_file)?
                != json!({"launch":self.launch.binding,"supervisor":supervisor,"process":identity})
        {
            return Err("Windows Redis 运行、授权或进程收据与原登记不同".into());
        }
        document::exact(
            &runtime["facts"],
            &[
                "process_id",
                "run_id",
                "config_file",
                "configuration_sha256",
            ],
            "Windows Redis 认证事实",
        )?;
        let run_id = &runtime["facts"]["run_id"];
        facts::check(request, identity, &runtime["facts"], run_id)?;
        let ack = json!({"process":receipt,"run_id":run_id});
        if document::read(&output.join("acknowledged.json"))? != ack
            || document::read(&output.join("accepted.json"))? != ack
            || runtime["redis"]
                != json!({
                    "platform":"windows","port":request["port"],"pid":identity["pid"],
                    "started":identity["started"],"executable":executable,"sha256":tool["sha256"],
                    "configuration":request["configuration"],"run_id":run_id,"tool":tool
                })
        {
            return Err("Windows Redis 认证接受或物理代次与登记不同".into());
        }
        if self.exited {
            self.check_exit()?;
        }
        Ok(())
    }

    fn check_running(&self) -> Result<()> {
        if self.exited {
            return Err("Windows Redis 已有退出收据，不能证明当前存活".into());
        }
        self.launch.check_supervisor(&self.runtime["supervisor"])?;
        let identity = &self.runtime["process"];
        let request = &self.launch.request;
        let executable = document::text(&request["tool"]["path"], "Windows Redis 工具路径")?;
        let port = u16::try_from(request["port"].as_u64().ok_or("Redis 端口无效")?)?;
        exact_arguments(identity, executable, &["redis.conf"])?;
        if !process::loopback_listener(identity, port)? {
            return Err("Windows Redis 登记进程未在原回环端点监听".into());
        }
        Ok(())
    }

    fn observe_running(&self) -> Result<Value> {
        self.check_running()?;
        let identity = &self.runtime["process"];
        let request = &self.launch.request;
        let observed = facts::bounded_windows(&self.launch.backend, request, identity)?;
        self.check_running()?;
        facts::check(
            request,
            identity,
            &observed,
            &self.runtime["redis"]["run_id"],
        )?;
        if observed != self.runtime["facts"] {
            return Err("Windows Redis 当前认证事实与原登记不同".into());
        }
        Ok(self.runtime.clone())
    }

    fn status(&self) -> Result<Value> {
        let supervisor = &self.runtime["supervisor"];
        let identity = &self.runtime["process"];
        let running = match (present(supervisor)?, present(identity)?) {
            (true, true) => {
                self.observe_running()?;
                true
            }
            (false, false) => {
                self.check_stopped()?;
                false
            }
            _ => return Err("Windows Redis 只有部分登记进程关闭，保留现场".into()),
        };
        Ok(json!({
            "state":if running { "running" } else { "stopped" },
            "launcher_alive":running,
            "windows":{"identity":identity,"alive":running}
        }))
    }

    fn check_exit(&self) -> Result<()> {
        if !self.exited {
            return Err("Windows Redis 缺少原 Job 完整退出收据".into());
        }
        let exited = document::read(&self.launch.output.join("exited.json"))?;
        document::exact(
            &exited,
            &["process", "returncode", "job_empty"],
            "Windows Redis 退出收据",
        )?;
        if exited["process"] != self.runtime["process_receipt"]
            || exited["job_empty"] != true
            || exited["returncode"]
                .as_i64()
                .is_none_or(|code| i32::try_from(code).is_err())
        {
            return Err("Windows Redis 原进程或 Job 完整退出未获证明".into());
        }
        Ok(())
    }

    fn check_stopped(&self) -> Result<()> {
        self.check_exit()?;
        self.check_closed_port()?;
        if present(&self.runtime["supervisor"])? || present(&self.runtime["process"])? {
            return Err("Windows Redis 端口核验后发现登记进程仍在运行".into());
        }
        Ok(())
    }

    fn external_termination(&self) -> Result<Value> {
        let supervisor = &self.runtime["supervisor"];
        let process = &self.runtime["process"];
        if !prove_external(self.exited, [supervisor, process], present, || {
            self.check_closed_port()
        })? {
            return Ok(Value::Null);
        }
        Ok(json!({"kind":"windows-redis-external-termination",
            "runtime":common::binding(&self.launch.output.join("runtime.json"))?,
            "supervisor":supervisor,"process":process,"port":self.launch.request["port"],
            "normal_job_exit_proof":null}))
    }

    fn check_closed_port(&self) -> Result<()> {
        let request = &self.launch.request;
        let port = request["port"].as_u64().ok_or("Redis 端口无效")?;
        let url = format!("http://127.0.0.1:{port}");
        authority::require_closed(&url)?;
        host_control::bind_and_close(request)?;
        authority::require_closed(&url)
    }

    fn finish(&self, result: Result<Value>) -> Result<Value> {
        let after = self.launch.check().and_then(|()| self.check_terminal());
        let result = match (result, after) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => {
                Err(format!("{error}；Windows Redis 原启动与关闭状态后验失败：{after}").into())
            }
        };
        self.launch_inputs
            .finish(self.receipt_inputs.finish(result))
    }
}

fn prove_external(
    exited: bool,
    identities: [&Value; 2],
    mut present: impl FnMut(&Value) -> Result<bool>,
    closed: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    if exited {
        return Ok(false);
    }
    let mut require_absent = || -> Result<()> {
        let [supervisor, process] = identities;
        if (present(supervisor)?, present(process)?) != (false, false) {
            return Err("Windows Redis 外部终止要求原监督器和 Redis 进程均已退出".into());
        }
        Ok(())
    };
    require_absent()?;
    closed()?;
    require_absent()?;
    Ok(true)
}

fn directory_files(output: &Path) -> Result<Vec<String>> {
    if digest::linked_path(output)?.contains(&true) {
        return Err("Windows Redis 运行目录不能经过链接".into());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(output)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Windows Redis 文件名不是 UTF-8")?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || digest::linked(&metadata) {
            return Err("Windows Redis 运行目录含链接或非普通文件".into());
        }
        files.push(name);
    }
    files.sort_unstable();
    Ok(files)
}

pub(super) fn entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn present(expected: &Value) -> Result<bool> {
    let pid = expected["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("Windows Redis 登记进程 PID 无效")?;
    match common::process_identity(pid)? {
        None => Ok(false),
        Some(actual) if actual == *expected => Ok(true),
        Some(_) => Err("Windows Redis 登记 PID 已复用，保留现场".into()),
    }
}

#[cfg(test)]
#[test]
fn external_termination_without_job_exit_requires_two_complete_absence_samples() {
    use std::cell::RefCell;

    let supervisor = json!({"pid":4});
    let process = json!({"pid":5});
    let identities = [&supervisor, &process];
    let events = RefCell::new(Vec::new());
    assert!(
        prove_external(
            false,
            identities,
            |identity| {
                events.borrow_mut().push(identity["pid"].as_u64().unwrap());
                Ok(false)
            },
            || {
                events.borrow_mut().push(0);
                Ok(())
            }
        )
        .unwrap()
    );
    assert_eq!(events.into_inner(), [4, 5, 0, 4, 5]);

    for changed_sample in 1..=4 {
        let mut sample = 0;
        assert!(
            prove_external(
                false,
                identities,
                |_| {
                    sample += 1;
                    Ok(sample == changed_sample)
                },
                || Ok(())
            )
            .is_err()
        );
    }
    assert!(prove_external(false, identities, |_| Err("身份变化".into()), || Ok(())).is_err());
    assert!(prove_external(false, identities, |_| Ok(false), || Err("端口占用".into())).is_err());
    assert!(!prove_external(true, identities, |_| unreachable!(), || unreachable!()).unwrap());
}

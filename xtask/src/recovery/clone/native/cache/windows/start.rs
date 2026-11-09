//! 授权和接受按原代次分开登记，失败只撤销本次自有监督器。
use super::*;
use crate::recovery::clone::native::cache::{
    diagnostic::Category, facts, host_control, request as cache_request, start_deadline::Deadline,
};
use std::time::{Duration, Instant};

pub(in crate::recovery::clone::native::cache) fn run(
    backend: &Path,
    request: &Value,
    private: &Value,
    output: &Path,
    mut guard: impl FnMut() -> Result<()>,
) -> Result<Value> {
    model::validate(backend, request, true)?;
    cache_request::run(
        "environment-select",
        &json!({"private":{"environment":private}}),
    )?;
    if !request["previous_identity"].is_null() {
        let previous = u32::try_from(
            request["previous_identity"]["pid"]
                .as_u64()
                .ok_or("Redis 前驱 PID 无效")?,
        )?;
        if common::process_identity(previous)?.is_some() {
            return Err("Windows Redis 前代 PID 仍存在或已复用".into());
        }
    }
    guard()?;
    let request_input = freeze_request(backend, output, request)?;
    host_control::wait_before_start(request, &mut guard)?;
    let launch = BoundLaunch::create(backend, output, request)?;
    publish(&output.join("intent.json"), &launch.binding)?;
    let protected = launch.protect()?;
    request_input.assert_unchanged()?;
    drop(request_input);
    guard()?;
    let timeout = Duration::from_secs(request["timeout_seconds"].as_u64().ok_or("缓存超时无效")?);
    let deadline = Deadline::new(Instant::now(), timeout);
    let executable = bound_path(&launch.coordinator, "path")?;
    let pid = crate::process::windows_detach::spawn_detached(&executable, &launch.arguments()?)?;
    let result = (|| {
        let supervisor =
            common::process_identity(pid)?.ok_or("Windows Redis 监督器创建身份缺失")?;
        launch.check_supervisor(&supervisor)?;
        deadline.check(Instant::now(), Category::ReadinessTimeout)?;
        publish(
            &output.join("authorized.json"),
            &json!({"launch":launch.binding,"supervisor":supervisor}),
        )?;
        let (runtime, acceptance) = ready(&launch, &supervisor, deadline, &mut guard)?;
        guard()?;
        protected.assert_unchanged()?;
        acceptance.check(Instant::now(), Category::AcceptanceTimeout)?;
        publish(&output.join("runtime.json"), &runtime)?;
        launch.check_supervisor(&supervisor)?;
        process::require(&runtime["process"])?;
        acceptance.check(Instant::now(), Category::AcceptanceTimeout)?;
        Ok(runtime)
    })();
    match protected.finish(result) {
        Ok(runtime) => Ok(runtime),
        Err(error) => match publish(&output.join("cancel.json"), &launch.binding) {
            Ok(()) => Err(error),
            Err(cancel) => Err(format!("{error}；本次 Windows Redis 撤销未登记：{cancel}").into()),
        },
    }
}

fn freeze_request(backend: &Path, output: &Path, request: &Value) -> Result<ProtectedInputs> {
    common::local_directory(backend, output, "Windows Redis 启动输出")?;
    let path = output.join("request.json");
    if !super::observe::entry_exists(&path)? {
        publish(&path, request)?;
    }
    let path = common::common_file(backend, &path, "Windows Redis 原请求")?;
    let protected = ProtectedInputs::open([path.clone()])?;
    protected.finish((|| {
        if document::read(&path)? != *request {
            return Err("Windows Redis 已有请求不同于本次冻结输入".into());
        }
        Ok(())
    })())?;
    Ok(protected)
}

fn ready(
    launch: &BoundLaunch,
    supervisor: &Value,
    deadline: Deadline,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<(Value, Deadline)> {
    let path = launch.output.join("process.json");
    wait_for_file(
        launch,
        supervisor,
        &path,
        deadline,
        Category::ProcessReceiptTimeout,
    )?;
    let process_receipt = common::binding(&path)?;
    let record = document::read(&path)?;
    document::exact(
        &record,
        &["launch", "supervisor", "process"],
        "Windows Redis 进程收据",
    )?;
    if record["launch"] != launch.binding
        || record["supervisor"] != *supervisor
        || common::binding(&path)? != process_receipt
    {
        return Err("Windows Redis 进程收据未绑定同一授权".into());
    }
    let identity = &record["process"];
    let request = &launch.request;
    let port = u16::try_from(request["port"].as_u64().ok_or("Redis 端口无效")?)?;
    let executable = document::text(&request["tool"]["path"], "Windows Redis 映像")?;
    loop {
        deadline.check(Instant::now(), Category::ReadinessTimeout)?;
        launch.check_supervisor(supervisor)?;
        exact_arguments(identity, executable, &["redis.conf"])?;
        if process::loopback_listener(identity, port)? {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let observed = facts::bounded_windows(&launch.backend, request, identity)?;
    facts::check(request, identity, &observed, &Value::Null)?;
    exact_arguments(identity, executable, &["redis.conf"])?;
    guard()?;
    launch.check()?;
    let acceptance = deadline.accepting(Instant::now(), Category::ReadinessTimeout)?;
    let ack = json!({"process":process_receipt,"run_id":observed["run_id"]});
    publish(&launch.output.join("acknowledged.json"), &ack)?;
    let path = launch.output.join("accepted.json");
    wait_for_file(
        launch,
        supervisor,
        &path,
        acceptance,
        Category::AcceptanceTimeout,
    )?;
    if document::read(&path)? != ack {
        return Err("Windows Redis 接受未绑定同一认证代次".into());
    }
    acceptance.check(Instant::now(), Category::AcceptanceTimeout)?;
    let runtime = json!({
        "format_version":3,"request_sha256":digest::plan_hash(request)?,
        "output":launch.output,"launch":launch.binding,"supervisor":supervisor,
        "process":identity,"process_receipt":process_receipt,"facts":observed,
        "redis":{"platform":"windows","port":request["port"],"pid":identity["pid"],
            "started":identity["started"],"executable":executable,"sha256":request["tool"]["sha256"],
            "configuration":request["configuration"],"run_id":observed["run_id"],"tool":request["tool"]}
    });
    Ok((runtime, acceptance))
}

fn wait_for_file(
    launch: &BoundLaunch,
    supervisor: &Value,
    path: &Path,
    deadline: Deadline,
    category: Category,
) -> Result<()> {
    loop {
        deadline.check(Instant::now(), category)?;
        if launch.output.join("failed.json").try_exists()? {
            return Err("Windows Redis 监督器记录了失败；保留原收据".into());
        }
        process::require(supervisor)?;
        if path.try_exists()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

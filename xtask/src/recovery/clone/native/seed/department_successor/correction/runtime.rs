//! 部门规划 readiness 嵌套运行时事故的单次接续，保留原修正及回收。
use super::{
    Generation, RegisteredPlan, Scope, common, controller, existing, fs, is_plan, number_field,
    old_producer, output, same_tool, stage_attempt, state,
};
use crate::{Result, recovery::clone::native::post_process::command};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const PANIC_LOCATION: &str = r") panicked at xtask\src\recovery\full_stack\worker\probes.rs:48:13:";
const PANIC_MESSAGE: &str = concat!(
    "Cannot start a runtime from within a runtime. This happens because a function (like ",
    "`block_on`) attempted to block the current thread while the thread is being used to drive ",
    "asynchronous tasks."
);
const PANIC_NOTE: &str =
    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace";

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join("seed-runtime/department-runtime-tool-correction.json")
}

impl Scope<'_> {
    pub(super) fn runtime_receipt(
        &self,
        first: u64,
        controller_binding: &Value,
        build_binding: &Value,
        tool: &Value,
    ) -> Result<Value> {
        let (previous, original) = existing(self.backend, &super::path(self.run))?
            .ok_or("部门运行时修正缺少原桥接修正登记")?;
        self.validate(&original, Generation::Bridge)?;
        let failed = number_field(&original, "first_attempt")?;
        let (failure_row, recover_row) = terminal_chain(
            self.attempts,
            number_field(self.api, "number")?,
            failed,
            first,
        )?;
        if [
            &original["tool"],
            &original["previous_tool"],
            &self.original.request["identity_tool"],
        ]
        .iter()
        .any(|prior| same_tool(prior, tool))
        {
            return Err("部门运行时修正必须使用独立的新冻结构建".into());
        }
        let proof =
            self.readiness_incident(failed, failure_row, recover_row, &previous, &original)?;
        Ok(json!({
            "format_version":1,"kind":"devex-seed-department-runtime-tool-correction",
            "run_manifest":state::manifest_binding(self.run)?,"first_attempt":first,
            "controller":controller_binding,"original_registration":self.original.descriptor,
            "identity_plan":self.original.request["identity_plan"],
            "api_publication":original["api_publication"],"api_result":self.api["result"],
            "previous_registration":previous,"previous_tool":original["tool"],
            "incident":proof,"build_request":build_binding,"tool":tool
        }))
    }

    fn readiness_incident(
        &self,
        number: u64,
        failed: &Value,
        recovery: &Value,
        previous: &Value,
        receipt: &Value,
    ) -> Result<Value> {
        let original_controller = controller(self.backend, self.run, failed)?;
        let failure = common::binding(&self.run.join(format!("failure-{number:04}.json")))?;
        if common::read_bound(self.backend, &failure, "部门运行时规划正式失败")?
            != json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
                "stage":"seed-runtime","mode":"departments-plan","error_type":"PostCopyCleanupRejected",
                "operation_id":"clone.seed-runtime.departments-plan","controller":original_controller})
        {
            return Err("部门 readiness 事故没有绑定原规划控制器和失败结果".into());
        }
        let directory = output(self.run, number);
        common::local_directory(self.backend, &directory, "原部门 readiness 事故目录")?;
        require_uninitialized(&directory)?;
        let launch = common::binding(&directory.join("session-launch.json"))?;
        let process = common::binding(&directory.join("session-process.json"))?;
        let request = super::project(self.original.request.clone(), receipt);
        old_producer(
            self.backend,
            self.run,
            number,
            &original_controller,
            &launch,
            &process,
            &RegisteredPlan {
                descriptor: previous,
                request: &request,
            },
        )?;
        self.require_launch_command(number, &directory, &launch, &process, &request)?;
        let stderr = self.panic_stderr(&directory)?;
        let logout = common::binding(&directory.join("department-logout-error.json"))?;
        if common::read_bound(self.backend, &logout, "原部门生产者退出错误")?
            != json!({"error_type":"ProducerLogoutError"})
        {
            return Err("部门 readiness 事故缺少原注销错误收据".into());
        }
        let recover_controller = controller(self.backend, self.run, recovery)?;
        let recovered = self.validate_recovery(recovery, &process)?;
        require_uninitialized(&directory)?;
        Ok(
            json!({"attempt":number,"controller":original_controller,"failure":failure,
            "launch":launch,"process":process,"stderr":stderr,"logout":logout,
            "recover_controller":recover_controller,"recovery":recovered}),
        )
    }

    fn require_launch_command(
        &self,
        number: u64,
        output: &Path,
        launch: &Value,
        process: &Value,
        request: &Value,
    ) -> Result<()> {
        let started = common::read_bound(self.backend, launch, "原 readiness 生产者启动")?;
        let observed = common::read_bound(self.backend, process, "原 readiness 生产者身份")?;
        let expected = command::build(&json!({"backend":self.backend,"run_dir":self.run,
            "attempt":number,"kind":"department-plan","output":output,
            "tool":request["identity_tool"],"registration":request,"configuration":null}))?;
        if started["command"] != expected["argv"]
            || started["source"] != expected["source"]
            || !observed["producer_owner"]
                .as_str()
                .is_some_and(|owner| uuid::Uuid::parse_str(owner).is_ok())
        {
            return Err("原 readiness 生产者参数、源码或所有者不同".into());
        }
        Ok(())
    }

    fn panic_stderr(&self, output: &Path) -> Result<Value> {
        let path = output.join("session.stderr.log");
        let binding = common::binding(&path)?;
        let recorded = common::bound_file(self.backend, &binding, "原部门 readiness panic")?;
        if recorded != path
            || number_field(&binding, "bytes")? > 1024
            || !is_readiness_panic(&fs::read_to_string(&recorded)?)
            || common::binding(&recorded)? != binding
        {
            return Err("原部门规划缺少同一生产者明确的 readiness 嵌套运行时 panic".into());
        }
        Ok(binding)
    }

    fn validate_recovery(&self, row: &Value, process: &Value) -> Result<Value> {
        let number = number_field(row, "number")?;
        let result = &row["result"];
        if common::bound_file(self.backend, result, "部门 readiness 会话回收")?
            != state::result_path(self.run, number)
            || common::read_bound(self.backend, result, "部门 readiness 零写入回收")?
                != json!({"status":"seed_session_recovered","producer":process,"already_stopped":true,
                    "copy_requires_reconciliation":true,"remote_writes":0,"restore_qualified":false})
        {
            return Err("旧部门 readiness 会话未证明原生产者已停止且远端零写入".into());
        }
        Ok(result.clone())
    }
}

fn terminal_chain(
    attempts: &[Value],
    api: u64,
    failed: u64,
    first: u64,
) -> Result<(&Value, &Value)> {
    if api >= failed || failed.checked_add(2) != Some(first) {
        return Err("部门运行时修正仅接续原修正的首轮规划和紧接的会话回收".into());
    }
    let failure = stage_attempt(attempts, failed)?;
    let recovery = stage_attempt(attempts, failed + 1)?;
    let plan = stage_attempt(attempts, first)?;
    if !is_plan(failure)
        || !is_plan(plan)
        || failure["status"] != "failed"
        || failure["error_type"] != "PostCopyCleanupRejected"
        || !failure["finished_at"].is_string()
        || !failure["result"].is_null()
        || recovery["stage"] != "seed-runtime"
        || recovery["mode"] != "recover-session"
        || recovery["status"] != "passed"
        || !recovery["finished_at"].is_string()
    {
        return Err("原部门运行时规划尚无明确失败及其后的正式回收".into());
    }
    Ok((failure, recovery))
}

fn require_uninitialized(output: &Path) -> Result<()> {
    for item in fs::read_dir(output)? {
        let name = item?.file_name();
        let name = name.to_str().ok_or("原部门 readiness 输出名称无效")?;
        if name == "authentication.json"
            || name == "pacing"
            || name.starts_with("http-")
            || name.ends_with("-before.json")
            || name.starts_with("department-") && name != "department-logout-error.json"
            || name.starts_with("session")
                && !matches!(
                    name,
                    "session-launch.json" | "session-process.json" | "session.stderr.log"
                )
        {
            return Err("原部门 readiness 事故已有初始化或后续业务证据，保留现场".into());
        }
    }
    Ok(())
}

fn is_readiness_panic(text: &str) -> bool {
    let mut lines = text.lines();
    if lines.next() != Some("") {
        return false;
    }
    // Windows panic 记录中的整数是线程标识，进程身份由原启动和回收绑定。
    let thread = lines
        .next()
        .and_then(|line| line.strip_prefix("thread 'main' ("))
        .and_then(|line| line.strip_suffix(PANIC_LOCATION));
    thread.is_some_and(|id| {
        id.bytes().all(|byte| byte.is_ascii_digit()) && id.parse::<u32>().is_ok_and(|id| id > 0)
    }) && lines.next() == Some(PANIC_MESSAGE)
        && lines.next() == Some(PANIC_NOTE)
        && lines.next().is_none()
}

#[cfg(test)]
#[test]
fn only_the_readiness_nested_runtime_panic_is_eligible() {
    let panic =
        format!("\r\nthread 'main' (55252{PANIC_LOCATION}\r\n{PANIC_MESSAGE}\r\n{PANIC_NOTE}\r\n");
    assert!(is_readiness_panic(&panic));
    assert!(!is_readiness_panic(
        &panic.replace("probes.rs:48", "probes.rs:49")
    ));
    assert!(!is_readiness_panic(&panic.replace("55252", "unknown")));
    assert!(!is_readiness_panic(&(panic + "additional diagnostic\n")));
}

#[cfg(test)]
#[test]
fn runtime_tool_requires_the_immediate_recovery_of_its_first_failed_plan() {
    let mut rows = [
        json!({"number":58,"stage":"seed-runtime","mode":"departments-plan","status":"failed",
            "finished_at":"2026-11-26T00:00:00Z","error_type":"PostCopyCleanupRejected","result":null}),
        json!({"number":59,"stage":"seed-runtime","mode":"recover-session","status":"passed",
            "finished_at":"2026-11-26T00:00:00Z"}),
        json!({"number":60,"stage":"seed-runtime","mode":"departments-plan","status":"running"}),
    ];
    assert!(terminal_chain(&rows, 55, 58, 60).is_ok());
    assert!(terminal_chain(&rows, 55, 56, 60).is_err());
    rows[1]["mode"] = json!("departments-verify");
    assert!(terminal_chain(&rows, 55, 58, 60).is_err());
    rows[1]["mode"] = json!("recover-session");
    rows[0]["status"] = json!("running");
    assert!(terminal_chain(&rows, 55, 58, 60).is_err());
}

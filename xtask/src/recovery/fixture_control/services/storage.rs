//! RustFS 专用监督启动；原请求、控制阶段和完整成员证明保持绑定。
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as document, resources::common},
        process_tree as platform,
    },
};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[path = "storage/actor.rs"]
mod actor;
#[path = "storage/binding.rs"]
pub(super) mod binding;
#[path = "storage/closed.rs"]
pub(super) mod closed;
#[path = "storage/launch.rs"]
mod launch;
#[path = "storage/monitor.rs"]
mod monitor;
#[path = "storage/records.rs"]
pub(super) mod records;
#[path = "storage/supervisor.rs"]
mod supervisor;

pub(super) struct Start<'a> {
    pub(super) backend: &'a Path,
    pub(super) request: &'a Value,
    pub(super) private: &'a Value,
    pub(super) output: &'a Path,
    pub(super) manifest: &'a Value,
    pub(super) controller: &'a Value,
    pub(super) number: u64,
}

pub(super) fn start(input: Start<'_>, mut guard: impl FnMut() -> Result<()>) -> Result<Value> {
    platform::supported()?;
    let actor = binding::prepare(&input)?;
    let arguments =
        crate::recovery::clone::native::resources::storage::model::arguments(input.request)?;
    let intent = json!({"format_version":1,"kind":"devex-clone-storage-intent",
        "request":input.manifest,"controller":input.controller,"attempt":input.number,"arguments":arguments});
    actor.verify_bound()?;
    guard()?;
    document::write_new(&input.output.join("intent.json"), &intent)?;
    let outcome: Result<Value> = (|| {
        actor.verify_bound()?;
        let tree = launch::start(&actor, &mut guard)?;
        ready(&actor, &tree, &mut guard)?;
        let storage = json!({"identity":tree["process"],"sha256":input.request["executable"]["sha256"],
            "process_receipt":common::binding(&input.output.join("process.json"))?,
            "launch_receipt":common::binding(&input.output.join("launch.json"))?,
            "tree":common::binding(&input.output.join("rustfs-tree.json"))?});
        actor.verify()?;
        guard()?;
        actor::running_tree(&actor, &tree)?;
        check(
            listening(&actor, &tree),
            "RustFS 最终完整核验后监听状态变化",
        )?;
        document::write_new(
            &input.output.join("ready.json"),
            &json!({"storage":storage,
            "request":input.manifest,"controller":input.controller,"attempt":input.number,
            "listeners":[input.request["api_url"],input.request["console_url"]]}),
        )?;
        Ok(storage)
    })();
    match outcome {
        Ok(value) => Ok(value),
        Err(error) => {
            let cleanup = failed(&actor);
            let reason = safe_reason(error.as_ref());
            Err(format!(
                "RustFS 原生监督启动未完成：{reason}；{}；保留全部收据，不自动重放",
                if cleanup.is_ok() {
                    "完整成员关闭已核对"
                } else {
                    "完整成员关闭未知"
                }
            )
            .into())
        }
    }
}

fn ready(actor: &actor::Actor, tree: &Value, guard: &mut impl FnMut() -> Result<()>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(actor.timeout()?);
    loop {
        check(Instant::now() < deadline, "RustFS 就绪期限耗尽")?;
        actor.verify_bound()?;
        guard()?;
        check(Instant::now() < deadline, "RustFS 就绪期限耗尽")?;
        actor::running_tree(actor, tree)?;
        let ready = listening(actor, tree);
        check(Instant::now() < deadline, "RustFS 就绪期限耗尽")?;
        if ready {
            return Ok(());
        }
        check(
            !actor.path("actor-failed.json").try_exists()?,
            "RustFS 监督启动已失败",
        )?;
        check(Instant::now() < deadline, "RustFS 未在期限内监听登记端口")?;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn listening(actor: &actor::Actor, tree: &Value) -> bool {
    ["api_url", "console_url"].iter().all(|key| {
        actor.request[key].as_str().is_some_and(|url| {
            super::lifecycle::readonly::process::listener(&tree["process"], url).is_ok()
        })
    })
}

fn failed(actor: &actor::Actor) -> Result<()> {
    if actor.path("rustfs-tree.json").try_exists()? {
        let tree = actor::tree(actor)?;
        return super::lifecycle::tree_control::stop(&actor.backend, &tree, true).map(|_| ());
    }
    // 未发布产品树时也只能请求本次 operation；monitor 持有真实成员，不由父进程猜 PID。
    document::write_new(
        &actor.path("actor-cancel.json"),
        &json!({"operation_id":actor.operation_id}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if actor.member_path("stopped").try_exists()? {
            let proof = document::read(&actor.member_path("stopped"))?;
            // 收据可能先于 monitor 的真实退出出现；只在原期限内继续只读等待。
            if actor::validate_members(actor, &proof).is_ok() {
                return Ok(());
            }
        }
        check(Instant::now() < deadline, "RustFS 部分启动关闭未知")?;
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug)]
struct Rejected(&'static str);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Rejected {}
pub(super) fn rejection_reason(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    error
        .downcast_ref::<Rejected>()
        .map(|error| error.0)
        .or_else(|| platform::rejection_reason(error))
}
pub(super) fn check(valid: bool, reason: &'static str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Box::new(Rejected(reason)))
    }
}
pub(super) fn safe_reason(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(reason) = rejection_reason(error) {
        return reason.into();
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return format!(
            "I/O kind={:?}, os_code={:?}",
            error.kind(),
            error.raw_os_error()
        );
    }
    "固定输入或监督原语核验未完成，细因未知".into()
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    actor::entry()
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/services/storage.rs"]
mod tests;

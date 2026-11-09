//! 在同一角色期限内核验完整进程身份、就绪内容和监听归属。
use std::{
    future::Future,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::super::{dataset_authority::check, generation::ROLES, probes};
use crate::{
    Result,
    recovery::{
        fixture_control::services::lifecycle::readonly::process,
        inputs::{document, domain},
    },
};

const MEMBERS: [&str; 3] = ["monitor", "supervisor", "process"];
const RETRY: Duration = Duration::from_millis(100);

pub(super) fn wait(
    tree: &Value,
    probe: &Value,
    timeout: Duration,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    check(
        !timeout.is_zero() && timeout <= Duration::from_secs(180),
        "恢复运行就绪超时必须在 0 到 180 秒之间",
    )?;
    let deadline = Instant::now() + timeout;
    let role = document::text(&probe["role"], "恢复运行探针角色")?;
    require_role(tree, role)?;
    let path = if role == "frontend" { "" } else { "/readyz" };
    let endpoint = domain::endpoint(&probe["url"], "恢复角色就绪端点", Some(path))?;
    check(
        endpoint.scheme == "http",
        "恢复角色必须使用登记的本机 HTTP 端点",
    )?;
    let address = endpoint.host.parse()?;
    probes::within_deadline(probe, deadline, |prepared| {
        attempts(
            deadline,
            move || {
                checkpoint()?;
                for member in MEMBERS {
                    probes::require_before(deadline)?;
                    process::require(&tree[member])?;
                }
                Ok(())
            },
            move || prepared.observe(),
            move || process::start_listener_at(&tree["process"], address, endpoint.port),
        )
    })
}

fn require_role(tree: &Value, role: &str) -> Result<()> {
    let [monitor, supervisor, product] = MEMBERS.map(|member| &tree[member]["pid"]);
    check(
        ROLES.contains(&role)
            && tree["role"] == role
            && tree["format_version"].as_u64() == Some(2)
            && tree["kind"] == "full-stack-process-tree"
            && tree["group_id"] == *supervisor
            && monitor != supervisor
            && supervisor != product
            && monitor != product,
        "恢复就绪探针与登记角色或完整成员树不一致",
    )
}

async fn attempts<F: Future<Output = Result<()>>>(
    deadline: Instant,
    mut guard: impl FnMut() -> Result<()>,
    mut observe: impl FnMut() -> F,
    mut listener: impl FnMut() -> Result<bool>,
) -> Result<()> {
    loop {
        probes::require_before(deadline)?;
        guard()?;
        probes::require_before(deadline)?;
        let observed = observe().await;
        probes::require_before(deadline)?;
        let listening = listener()?;
        probes::require_before(deadline)?;
        // 只有内容或监听未就绪可以重试，父检查或内核创建身份不明立即退出。
        guard()?;
        probes::require_before(deadline)?;
        if observed.is_ok() && listening {
            return Ok(());
        }
        tokio::time::sleep(RETRY).await;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/start/readiness.rs"]
mod tests;

//! 生产者执行期间固定登记、post-copy 代次及存活控制器；不把文件证明当成资源守卫。
use super::{Registration, history, identity};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{identity as process, state},
        resources::common,
    },
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) struct Current {
    registration: Registration,
    post: history::Post,
    plan: Value,
    controller: Value,
    run: PathBuf,
}
impl Current {
    pub(crate) fn load(backend: &Path, run: &Path) -> Result<Self> {
        let registration = Registration::load(backend, run, false)?;
        let post = history::validate(backend, run, &registration.value)?;
        let plan = identity::inputs(backend, run, &registration.value, &post)?;
        let controller = controller(backend, run, &registration.state)?;
        let current = Self {
            registration,
            post,
            plan,
            controller,
            run: run.to_path_buf(),
        };
        current.check(backend)?;
        Ok(current)
    }
    pub(crate) fn check(&self, backend: &Path) -> Result<()> {
        self.registration.assert_unchanged(backend, false)?;
        let post = history::validate(backend, &self.run, &self.registration.value)?;
        if post.request != self.post.request
            || post.descriptor != self.post.descriptor
            || post.runtime != self.post.runtime
            || identity::inputs(backend, &self.run, &self.registration.value, &post)? != self.plan
            || controller(backend, &self.run, &self.registration.state)? != self.controller
        {
            return Err("seed 生产者执行期间登记、当前 post-copy 或控制器代次变化；保留原意图，禁止自动重放".into());
        }
        self.registration.assert_unchanged(backend, false)
    }
}

fn controller(backend: &Path, run: &Path, saved: &Value) -> Result<Value> {
    let attempt = state::running_attempt(saved)?.ok_or("seed 生产者缺少当前执行阶段")?;
    if attempt["stage"] != "seed-runtime"
        || !matches!(
            attempt["mode"].as_str(),
            Some(
                "identities-apply"
                    | "identities-verify"
                    | "quotas-plan"
                    | "quotas-apply"
                    | "quotas-reconcile"
                    | "departments-plan"
                    | "departments-apply"
                    | "departments-reconcile"
                    | "departments-verify"
                    | "prepare"
                    | "start"
                    | "close"
            )
        )
    {
        return Err("seed 生产者当前阶段或模式无效".into());
    }
    let number = attempt["number"]
        .as_u64()
        .ok_or("seed 生产者阶段序号无效")?;
    let descriptor = common::binding(&state::controller_path(run, number))?;
    let controller = common::read_bound(backend, &descriptor, "seed 当前控制器")?;
    let owner_path = run.join("run.lock").join("owner.json");
    let owner = common::read_bound(backend, &common::binding(&owner_path)?, "seed 当前运行锁")?;
    document::exact(
        &controller,
        &[
            "format_version",
            "kind",
            "owner",
            "attempt",
            "attempt_sha256",
        ],
        "seed 当前控制器",
    )?;
    process::validate(&owner, run)?;
    let pid = owner["identity"]["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("seed 控制器 PID 无效")?;
    if controller["format_version"].as_u64() != Some(1)
        || controller["kind"] != "devex-stage-controller"
        || controller["attempt"] != number
        || controller["attempt_sha256"].as_str() != Some(digest::plan_hash(attempt)?.as_str())
        || controller["owner"] != owner
        || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
        || process::process(pid)?.as_ref() != Some(&owner["identity"])
    {
        return Err("seed 生产者控制器已退出或不持有当前阶段运行锁".into());
    }
    Ok(descriptor)
}

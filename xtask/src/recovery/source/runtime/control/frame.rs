//! generation 控制器复用统一阶段持锁区间，不认领代理 owner，也不重新取得运行锁。
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        json as document,
        lifecycle::{identity, manifest, stage::StageRequest, state},
        resources::common,
        source,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) struct Frame<'a> {
    pub backend: &'a Path,
    pub run: &'a Path,
    pub number: u64,
    pub mode: &'a str,
    pub prefix: Vec<Value>,
    pub scope: Scope,
    manifest: manifest::Manifest,
    saved: Value,
    state_binding: Value,
    controller: Value,
    owner: Value,
    owner_binding: Value,
    control_source: Value,
}

impl<'a> Frame<'a> {
    pub(super) fn state(&self) -> &Value {
        &self.saved
    }

    pub(super) fn source(&self) -> &Value {
        &self.control_source
    }

    pub(super) fn load(request: &'a StageRequest<'a>) -> Result<Self> {
        let number = request
            .attempt
            .filter(|n| *n > 0)
            .ok_or("源代次控制缺少实际 attempt")?;
        stage(request)?;
        let backend = request.backend;
        let run = request.run_dir;
        let manifest = manifest::load(backend, &run.join("manifest.json"))?;
        if manifest.value["copy_stage"] != "source_to_seed" {
            return Err("源代次控制只能消费 source_to_seed 的完整运行清单".into());
        }
        let state_binding = common::binding(&run.join("state.json"))?;
        let saved = state::load(run, true)?;
        let attempt = active(&saved, number, request.mode)?;
        let (controller, owner) = state::controller_record(run, number, attempt)?;
        owner_identity(&owner, run, &manifest.digest.sha256)?;
        let control_source = source::current(backend)?;
        if control_source != attempt["sources"] {
            return Err(
                "源控制器的当前完整工具来源与实际运行阶段登记不同，禁止执行资源操作".into(),
            );
        }
        let owner_binding = common::binding(&run.join("run.lock/owner.json"))?;
        if common::read_bound(backend, &owner_binding, "源控制锁持有者")? != owner {
            return Err("源代次控制器与当前运行锁的完整 owner 不同".into());
        }
        let attempts = state::attempts(&saved)?;
        let prefix = attempts[..attempts.len() - 1].to_vec();
        let scope = Scope::load(
            backend,
            document::array(&manifest.value["build_bridges"], "源控制构建桥接")?,
            &[],
        )?;
        let frame = Self {
            backend,
            run,
            number,
            mode: request.mode,
            prefix,
            scope,
            manifest,
            saved,
            state_binding,
            controller,
            owner,
            owner_binding,
            control_source,
        };
        frame.checkpoint()?;
        Ok(frame)
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        let manifest = manifest::load(self.backend, &self.run.join("manifest.json"))?;
        let saved = state::load(self.run, true)?;
        let attempt = active(&saved, self.number, self.mode)?;
        let (controller, owner) = state::controller_record(self.run, self.number, attempt)?;
        owner_identity(&owner, self.run, &manifest.digest.sha256)?;
        if manifest.value != self.manifest.value
            || manifest.digest != self.manifest.digest
            || saved != self.saved
            || common::binding(&self.run.join("state.json"))? != self.state_binding
            || controller != self.controller
            || owner != self.owner
            || common::read_bound(self.backend, &self.owner_binding, "源控制锁持有者")? != owner
            || common::binding(&self.run.join("run.lock/owner.json"))? != self.owner_binding
            || source::current(self.backend)? != self.control_source
        {
            return Err(
                "源代次停止或恢复期间清单、完整阶段前缀、控制器或工具来源变化；保留现场，禁止重放"
                    .into(),
            );
        }
        self.scope.assert_unchanged()
    }

    pub(super) fn verifier_stopped(&self) -> Result<Value> {
        self.checkpoint()?;
        let starts = super::super::running::history::starts(self.backend, self.run, &self.prefix)?;
        let [start] = starts.as_slice() else {
            return Err("回收必须绑定唯一完整来源启动记录".into());
        };
        let result = if start["status"] == "passed" {
            super::super::stopped::require(self.backend, &start["result"])?
        } else {
            let number = start["number"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or("原来源启动序号无效")?;
            let output = self.run.join(format!("g{number:04}"));
            super::super::running::directory(self.backend, &output)?;
            if output.join("verification").try_exists()? {
                return Err(
                    "未成功发布的来源启动存在未知验收生产者证据；禁止猜测退出或回收".into(),
                );
            }
            json!({"status":"not_started","process":null})
        };
        self.checkpoint()?;
        Ok(result)
    }
}

pub(super) fn stage(request: &StageRequest<'_>) -> Result<()> {
    if request.stage != "seed-runtime"
        || !matches!(
            request.mode,
            "source-generation-stop" | "source-generation-recover"
        )
    {
        return Err("源代次控制只接受正式 seed-runtime stop/recover 阶段".into());
    }
    Ok(())
}

fn active<'a>(saved: &'a Value, number: u64, mode: &str) -> Result<&'a Value> {
    let attempt = state::running_attempt(saved)?.ok_or("源代次控制缺少当前运行阶段")?;
    if number == 0
        || attempt["number"].as_u64() != Some(number)
        || attempt["stage"] != "seed-runtime"
        || attempt["mode"] != mode
        || state::attempts(saved)?.last() != Some(attempt)
    {
        return Err("源代次停止或恢复必须属于同一账本唯一当前持锁阶段".into());
    }
    Ok(attempt)
}

fn owner_identity(owner: &Value, run: &Path, manifest_sha: &str) -> Result<()> {
    identity::validate(owner, run)?;
    if owner["manifest_sha256"] != manifest_sha || owner["identity"] != identity::current()? {
        return Err(
            "源代次控制要求实际当前原生进程持有同一清单的运行锁；不能代理或跨进程授权".into(),
        );
    }
    Ok(())
}

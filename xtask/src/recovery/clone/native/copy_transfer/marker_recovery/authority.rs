use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    super::{
        super::{
            copy_ledger::{self, FailedMarkerJournal, session::Mode},
            digest, export, json as document,
            lifecycle::state,
            resources::common,
            target_verify::guard::Inputs,
        },
        authority::{current_controller, export_descriptor},
        proofs::Proofs,
        record::{Facts, Files},
        steps::Plan,
    },
    Checkpoint, Generation, ProcessGuard, StageRequest, Storage, Target, absent, protected_file,
};
use crate::Result;

#[path = "authority/failed_copy.rs"]
mod failed_copy;
mod successor;
use failed_copy::{FailedCopy, is_failed_reconcile};

pub(super) fn preflight(request: &StageRequest<'_>, manifest: &Value) -> Result<()> {
    if !cfg!(windows)
        || request.stage != "target-cache-markers"
        || !matches!(request.mode, "run" | "reconcile")
        || manifest["copy_stage"] != "source_to_seed"
    {
        return Err("Redis 标记恢复只支持 Windows 原 source_to_seed 的 run 或 reconcile".into());
    }
    Ok(())
}

pub(super) struct Stage<'a> {
    pub(super) request: &'a StageRequest<'a>,
    pub(super) manifest: &'a Value,
    pub(super) target: PathBuf,
    pub(super) ledger: PathBuf,
    pub(super) attempt: u64,
    pub(super) controller: Value,
    manifest_binding: Value,
    saved: Value,
    protected: Inputs,
    failed_reconcile: Option<Value>,
}

impl<'a> Stage<'a> {
    pub(super) fn load(request: &'a StageRequest<'a>, manifest: &'a Value) -> Result<Self> {
        let backend = request.backend;
        let run = request.run_dir;
        let attempt = request.attempt.ok_or("Redis 标记恢复缺少正式阶段序号")?;
        let paths = [
            run.join("manifest.json"),
            run.join("state.json"),
            state::controller_path(run, attempt),
            run.join("run.lock/owner.json"),
        ];
        let protected = hold(backend, &paths)?;
        let saved = state::load(run, true)?;
        if document::read(&paths[0])? != *manifest {
            return Err("Redis 标记恢复运行清单变化".into());
        }
        let (controller, _) =
            current_controller(run, &saved, attempt, request.stage, request.mode)?;
        // 失败核对的候选只读预检到此，真实 Redis 单独后继须由 Recovery::load 证明。
        let (prior, target_verified) = eligible_attempts(state::attempts(&saved)?, true)?;
        if prior["mode"] == "reconcile" && prior["status"] == "passed" {
            let result = common::read_bound(backend, &prior["result"], "原复制只读核对")?;
            if result["status"] != "copy_reconciled" || result["target_ready"] != false {
                return Err("原复制核对未确认可继续的未完复制".into());
            }
        }
        let verified = common::read_bound(backend, &target_verified["result"], "原复制目标复验")?;
        if verified["status"] != "fresh_target_reverified"
            || verified["initialized"] != manifest["initialized"]
        {
            return Err("目标复验与初始化证明不同".into());
        }
        for path in ["post-copy.json", "seed-runtime.json"] {
            absent(&run.join(path))?;
        }
        let initialized = common::bound_file(backend, &manifest["initialized"], "原 Redis 初始化")?;
        let target = initialized
            .parent()
            .ok_or("原目标初始化缺少目录")?
            .to_owned();
        let copy = document::text(&manifest["copy_directory"], "原复制目录")?;
        let copy = common::local_directory(backend, Path::new(copy), "原复制目录")?;
        absent(&copy.join("result.json"))?;
        let ledger = common::local_directory(backend, &copy.join("ledger"), "原复制账本")?;
        protected.check()?;
        let manifest_binding = common::binding(&paths[0])?;
        let failed_reconcile = is_failed_reconcile(prior).then(|| prior.clone());
        Ok(Self {
            request,
            manifest,
            target,
            ledger,
            attempt,
            controller,
            manifest_binding,
            saved,
            protected,
            failed_reconcile,
        })
    }

    pub(super) fn verify_intent_controller(
        backend: &Path,
        run: &Path,
        number: u64,
        expected: &Value,
    ) -> Result<()> {
        let path = protected_file(backend, &state::controller_path(run, number))?;
        let held = Inputs::open([path])?;
        let saved = state::load(run, true)?;
        let mut attempt = state::attempts(&saved)?
            .iter()
            .find(|row| row["number"].as_u64() == Some(number))
            .ok_or("Redis 标记意图未绑定原运行阶段")?
            .clone();
        if attempt["stage"] != "target-cache-markers"
            || attempt["mode"] != "run"
            || !matches!(attempt["status"].as_str(), Some("failed" | "passed"))
        {
            return Err("Redis 标记意图不属于此前的正式写入阶段".into());
        }
        attempt["status"] = json!("running");
        for field in ["finished_at", "result", "error_type"] {
            attempt[field] = Value::Null;
        }
        if state::controller_record(run, number, &attempt)?.0 != *expected {
            return Err("Redis 标记意图绑定的原控制器收据变化".into());
        }
        held.check()
    }

    fn check(&self) -> Result<()> {
        self.protected.check()?;
        let request = self.request;
        let (controller, _) = current_controller(
            request.run_dir,
            &self.saved,
            self.attempt,
            request.stage,
            request.mode,
        )?;
        if controller != self.controller {
            return Err("Redis 标记恢复期间运行控制器变化".into());
        }
        for name in ["post-copy.json", "seed-runtime.json"] {
            absent(&request.run_dir.join(name))?;
        }
        let copy = self.ledger.parent().ok_or("原复制账本缺少目录")?;
        absent(&copy.join("result.json"))?;
        absent(&self.ledger.join("clone.lock"))
    }
}

fn eligible_attempts(
    attempts: &[Value],
    consider_failed_reconcile: bool,
) -> Result<(&Value, &Value)> {
    let copy = attempts.iter().rfind(|row| row["stage"] == "copy");
    let Some(copy) = copy.filter(|row| {
        (row["mode"] == "run"
            && row["status"] == "failed"
            && row["error_type"] == "ControllerInterrupted")
            || (row["mode"] == "reconcile" && row["status"] == "passed")
            || (consider_failed_reconcile && is_failed_reconcile(row))
    }) else {
        return Err("Redis 标记恢复要求原复制中断或只读核对已发布".into());
    };
    let verified = attempts.iter().find(|row| {
        row["stage"] == "target-verify" && row["status"] == "passed"
            && matches!((row["number"].as_u64(), copy["number"].as_u64()), (Some(a), Some(b)) if a < b)
    }).ok_or("Redis 标记恢复缺少早于原复制的目标复验")?;
    if is_failed_reconcile(copy) && !attempts.iter().any(|row| {
        row["stage"] == "copy"
            && matches!(row["mode"].as_str(), Some("run" | "resume"))
            && row["status"] == "failed"
            && matches!(
                (verified["number"].as_u64(), row["number"].as_u64(), copy["number"].as_u64()),
                (Some(verified), Some(write), Some(reconcile)) if verified < write && write < reconcile
            )
    }) {
        return Err("失败只读核对缺少原目标复验之后的复制写入阶段".into());
    }
    if attempts.iter().any(|row| {
        matches!(
            row["stage"].as_str(),
            Some("runtime-target" | "post-copy" | "seed-runtime" | "cache-target")
        ) || row["stage"] == "copy" && row["status"] == "passed" && row["mode"] != "reconcile"
    }) {
        return Err("已有目标业务、替代缓存或成功复制历史，拒绝恢复标记".into());
    }
    Ok((copy, verified))
}

pub(super) struct Recovery<'a> {
    pub(super) stage: &'a Stage<'a>,
    pub(super) binding: Value,
    pub(super) head: Value,
    pub(super) after: Value,
    storage: &'a Storage,
    owner: &'a Generation,
    fence: &'a ProcessGuard,
    target: &'a Target,
    protected: Inputs,
    proof: Proofs,
    plan: Inputs,
    source: Source,
    failed_copy: Option<FailedCopy>,
}

impl<'a> Recovery<'a> {
    pub(super) fn load(
        stage: &'a Stage<'a>,
        storage: &'a Storage,
        owner: &'a Generation,
        fence: &'a ProcessGuard,
        target: &'a Target,
    ) -> Result<Self> {
        let backend = stage.request.backend;
        let paths = [
            storage.directory.join("manifest.json"),
            storage.directory.join("state.json"),
        ];
        let fixture_guard = Checkpoint::FixtureSuccessor.check(hold(backend, &paths))?;
        let fixture = Checkpoint::FixtureSuccessor.check(successor::load(
            backend,
            &storage.directory,
            target,
        ))?;
        let redis_only = fixture.value["lineage"]
            .get("accepted_redis_failure")
            .is_some();
        if stage.failed_reconcile.is_some() && !redis_only {
            return Checkpoint::FixtureSuccessor.check(Err(
                "原失败复制核对只允许已验证的 Redis 单独后继恢复标记".into(),
            ));
        }
        Checkpoint::MarkerModel.check(require_marker_model(target))?;
        Checkpoint::FixtureSuccessor.check(fixture_guard.check())?;
        let mut files = fixture.files.clone();
        files.extend(paths);
        files.extend([
            stage.ledger.join("events.ndjson"),
            stage.ledger.join("head.json"),
        ]);
        let protected = Checkpoint::FinalGuards.check(hold(backend, &files))?;
        let current_fixture = Checkpoint::FixtureSuccessor.check(successor::load(
            backend,
            &storage.directory,
            target,
        ))?;
        if current_fixture != fixture {
            return Checkpoint::FixtureSuccessor
                .check(Err("Redis 标记恢复固定代次期间夹具证据变化".into()));
        }
        let CopyJournal {
            head,
            failed,
            proof,
            inputs: plan,
            source,
        } = Checkpoint::CopyProof.check(journal(stage, target))?;
        Checkpoint::FinalGuards.check(plan.check())?;
        let binding = json!({
            "run":stage.manifest_binding, "initialized":stage.manifest["initialized"],
            "fixture":fixture.value, "plan_sha256":head["plan_sha256"],
            "generation_sha256":head["generation_sha256"],
        });
        let recovered = Self {
            stage,
            binding,
            head,
            after: target.initialized["redis"].clone(),
            storage,
            owner,
            fence,
            target,
            protected,
            proof,
            plan,
            source,
            failed_copy: failed,
        };
        Checkpoint::FinalGuards.check(recovered.check())?;
        Ok(recovered)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.stage.check()?;
        self.storage.check()?;
        self.owner.check_initialization_lock()?;
        self.fence.check()?;
        self.protected.check()?;
        if let Some(failed) = &self.failed_copy {
            failed.bound.check()?;
        }
        self.proof.checkpoint()?;
        self.plan.check()?;
        self.source.check(self.stage)?;
        self.target.check_generation(self.owner)
    }

    pub(super) fn check_prefix(&self, head: &Value) -> Result<()> {
        if self.failed_copy.is_some() {
            if *head != self.head {
                return Err("本代标记意图未绑定同一失败核对账本 head".into());
            }
            return Ok(());
        }
        let backend = self.stage.request.backend;
        let plan = document::text(&self.binding["plan_sha256"], "原复制计划摘要")?;
        let generation = document::text(&self.binding["generation_sha256"], "原复制代次摘要")?;
        let (current, _) =
            copy_ledger::marker_prefix(backend, &self.stage.ledger, plan, generation, Some(head))?;
        if current != self.head {
            return Err("标记恢复期间复制账本发生追加".into());
        }
        Ok(())
    }

    pub(super) fn check_previous_prefix(&self, head: &Value) -> Result<()> {
        if let Some(failed) = &self.failed_copy {
            failed.journal.verify_previous(head)
        } else {
            self.check_prefix(head)
        }
    }
}

struct Source {
    held: Inputs,
    producers: Vec<Value>,
    ports: [String; 2],
    tool: PathBuf,
    generation: Value,
}

impl Source {
    fn load(stage: &Stage<'_>, export: &Value) -> Result<Self> {
        let backend = stage.request.backend;
        let descriptor = &stage.manifest["source_request"];
        let request_path = common::bound_file(backend, descriptor, "原复制来源请求")?;
        let request = document::read(&request_path)?;
        let root = Path::new(document::text(&export["artifact_root"], "原导出目录")?);
        let proof = &export["evidence"]["generation-after"];
        let path = digest::safe_file(root, document::text(&proof["file"], "原导出停止证明")?)?;
        let runtime_path = common::bound_file(backend, &request["runtime"], "原源运行收据")?;
        let registry_path =
            common::bound_file(backend, &request["producers_registry"], "原生产者登记")?;
        let mut paths = vec![
            request_path,
            path.clone(),
            runtime_path.clone(),
            registry_path,
        ];
        for role in ["api", "worker"] {
            paths.push(common::bound_file(
                backend,
                &request["processes"][role],
                "原生产者收据",
            )?);
        }
        let held = hold(backend, &paths)?;
        let binding = digest::file(&path)?;
        if proof["sha256"] != binding.sha256
            || proof["bytes"] != binding.bytes
            || common::read_bound(backend, descriptor, "原复制来源请求后像")? != request
        {
            return Err("原导出停止证明或生产者请求摘要不同".into());
        }
        let generation = document::read(&path)?;
        let observed = export::proof::copy::request(
            &json!({"backend":backend,"run_dir":stage.request.run_dir}),
        )?;
        if generation != observed {
            return Err("冻结来源与当前生产者停止证明不同".into());
        }
        let producers = document::array(&observed["operator_declared_producers"], "原生产者")?
            .iter()
            .map(|entry| entry["identity"].clone())
            .collect();
        let runtime = document::read(&runtime_path)?;
        let ports = [
            document::text(&request["source"]["api_url"], "源 API 端口")?.to_owned(),
            document::text(&runtime["worker_ready_url"], "源 Worker 端口")?.to_owned(),
        ];
        let tool = crate::recovery::fixture_control::portable_path(&std::env::current_exe()?);
        held.check()?;
        Ok(Self {
            held,
            producers,
            ports,
            tool,
            generation,
        })
    }

    fn check(&self, stage: &Stage<'_>) -> Result<()> {
        self.held.check()?;
        for producer in &self.producers {
            crate::recovery::clone::native::seed::source_registration::quiet::require_stopped(
                stage.request.backend,
                stage.request.run_dir,
                producer,
                &self.tool,
            )?;
        }
        for port in &self.ports {
            crate::recovery::clone::native::target_verify::authority::require_closed(port)?;
        }
        self.held.check()
    }
}

struct CopyJournal {
    head: Value,
    failed: Option<FailedCopy>,
    proof: Proofs,
    inputs: Inputs,
    source: Source,
}

fn journal(stage: &Stage<'_>, target: &Target) -> Result<CopyJournal> {
    let backend = stage.request.backend;
    let descriptor =
        export_descriptor(backend, stage.request.run_dir, stage.manifest, &stage.saved)?;
    let files = Files::bind(backend, stage.manifest, &descriptor)?;
    let paths = ["input.json", "plan.json", "session.json"].map(|name| files.root.join(name));
    let guard = hold(backend, &paths)?;
    let input = document::read(&paths[0])?;
    let (wrapper, plan_guard) = files.read_plan(&input)?;
    let saved = files.read_record()?;
    let export = common::read_bound(backend, &descriptor, "原复制导出")?;
    let source_root = Path::new(document::text(&export["artifact_root"], "原导出目录")?);
    let source_proof = &export["evidence"]["generation-after"];
    let proof = Proofs::open(
        source_root,
        &input["evidence"],
        source_proof,
        files.initialized,
        &target.initialized,
    )?;
    let source = Checkpoint::SourceGeneration.check(Source::load(stage, &export))?;
    let verified = super::super::super::issue(backend, &paths[1], &wrapper)?;
    let plan = &verified["plan"];
    let facts = Facts {
        source_generation: &source.generation,
        target_generation: &target.initialized["generation"],
        source_observation: &saved["source_observation"],
        declaration: &input,
    };
    if files.value(&facts, plan)? != saved
        || plan["copy_id"] != stage.manifest["id"]
        || plan["copy_stage"] != "source_to_seed"
        || plan["target_scope"] != target.request["target"]["scope_id"]
    {
        return Err("复制计划、会话及原初始化代次不同".into());
    }
    let plan_hash = document::text(&saved["plan_sha256"], "原复制计划摘要")?;
    let generation = document::text(&saved["generation_sha256"], "原复制代次摘要")?;
    let plan = Plan::from_verified(plan)?;
    let failed = stage.failed_reconcile.as_ref();
    let (head, failed) = Checkpoint::LedgerPrefix.check(if let Some(row) = failed {
        FailedMarkerJournal::load(backend, &stage.ledger, plan_hash, generation).and_then(
            |journal| {
                validate_plan_steps(&plan, journal.steps(), Mode::Reconcile)?;
                let head = journal.head().clone();
                Ok((head, Some(FailedCopy::bind(stage, row, &files, journal)?)))
            },
        )
    } else {
        copy_ledger::marker_prefix(backend, &stage.ledger, plan_hash, generation, None).and_then(
            |(head, steps)| {
                validate_plan_steps(&plan, &steps, Mode::Resume)?;
                Ok((head, None))
            },
        )
    })?;
    plan_guard.check()?;
    guard.check()?;
    Ok(CopyJournal {
        head,
        failed,
        proof,
        inputs: guard,
        source,
    })
}

fn validate_plan_steps(
    plan: &Plan<'_>,
    steps: &serde_json::Map<String, Value>,
    mode: Mode,
) -> Result<()> {
    plan.validate_states(steps, mode)?;
    if steps.len() >= plan.all().count()
        || plan.all().any(|step| {
            steps
                .get(&step.id)
                .is_some_and(|value| value["artifact_sha256"] != step.artifact)
        })
    {
        return Err("原复制账本已完成或步骤不是原计划绑定的产物".into());
    }
    Ok(())
}

fn require_marker_model(target: &Target) -> Result<()> {
    let scope = document::text(&target.request["target"]["scope_id"], "Redis scope")?;
    common::scope_id(scope)?;
    let owner = format!("ryframe:{{{scope}}}:.ryframe-owner");
    let sentinel = &target.request["reset"]["sentinel_value"];
    let id = document::text(&target.request["id"], "目标初始化 ID")?;
    if target.request["side"] != "seed"
        || target.request["reset"]["sentinel_key"]
            != format!("ryframe:devex-fresh:{scope}:sentinel")
        || *sentinel != format!("devex-fresh:{id}")
        || target.initialized["redis"]
            != json!({"keys":[owner],"owner":format!("ryframe-owner:v1:{scope}:redis"),"sentinel":sentinel})
    {
        return Err("目标 Redis 完整后像必须来自原成功初始化的两个标记".into());
    }
    Ok(())
}

fn hold(backend: &Path, paths: &[PathBuf]) -> Result<Inputs> {
    let paths = paths
        .iter()
        .map(|path| protected_file(backend, path))
        .collect::<Result<Vec<_>>>()?;
    Inputs::open(paths)
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/copy_transfer/marker_recovery_authority.rs"]
mod tests;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    super::{
        super::{
            copy_ledger::{self, session::Mode},
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
    Generation, ProcessGuard, StageRequest, Storage, Target, absent, protected_file,
};
use crate::{Result, recovery::fixture_control::services};

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
        let (prior, target_verified) = eligible_attempts(state::attempts(&saved)?)?;
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

fn eligible_attempts(attempts: &[Value]) -> Result<(&Value, &Value)> {
    let copy = attempts.iter().rfind(|row| row["stage"] == "copy");
    let Some(copy) = copy.filter(|row| {
        (row["mode"] == "run"
            && row["status"] == "failed"
            && row["error_type"] == "ControllerInterrupted")
            || (row["mode"] == "reconcile" && row["status"] == "passed")
    }) else {
        return Err("Redis 标记恢复要求原复制中断或只读核对已发布".into());
    };
    let verified = attempts.iter().find(|row| {
        row["stage"] == "target-verify" && row["status"] == "passed"
            && matches!((row["number"].as_u64(), copy["number"].as_u64()), (Some(a), Some(b)) if a < b)
    }).ok_or("Redis 标记恢复缺少早于原复制的目标复验")?;
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
        let fixture_guard = hold(backend, &paths)?;
        let fixture = fixture_successor(backend, &storage.directory, target)?;
        require_marker_model(target)?;
        let (heads, proof, plan, source) = journal(stage, target)?;
        fixture_guard.check()?;
        plan.check()?;
        let protected = hold(
            backend,
            &[
                paths[0].clone(),
                paths[1].clone(),
                common::bound_file(backend, &fixture["restart"], "原夹具重启")?,
                common::bound_file(backend, &fixture["generation"], "原夹具重启代次")?,
                common::bound_file(backend, &fixture["termination"], "原夹具外部终止")?,
                common::bound_file(
                    backend,
                    &fixture["termination_evidence"],
                    "原 Redis 外部终止",
                )?,
                stage.ledger.join("events.ndjson"),
                stage.ledger.join("head.json"),
            ],
        )?;
        let binding = json!({
            "run":stage.manifest_binding, "initialized":stage.manifest["initialized"],
            "fixture":fixture, "plan_sha256":heads["plan_sha256"],
            "generation_sha256":heads["generation_sha256"],
        });
        let recovered = Self {
            stage,
            binding,
            head: heads,
            after: target.initialized["redis"].clone(),
            storage,
            owner,
            fence,
            target,
            protected,
            proof,
            plan,
            source,
        };
        recovered.check()?;
        Ok(recovered)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.stage.check()?;
        self.storage.check()?;
        self.owner.check_initialization_lock()?;
        self.fence.check()?;
        self.protected.check()?;
        self.proof.checkpoint()?;
        self.plan.check()?;
        self.source.check(self.stage)?;
        self.target.check_generation(self.owner)
    }

    pub(super) fn check_prefix(&self, head: &Value) -> Result<()> {
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
            registry_path.clone(),
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

fn journal(stage: &Stage<'_>, target: &Target) -> Result<(Value, Proofs, Inputs, Source)> {
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
    let source = Source::load(stage, &export)?;
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
    let (head, steps) =
        copy_ledger::marker_prefix(backend, &stage.ledger, plan_hash, generation, None)?;
    let plan = Plan::from_verified(plan)?;
    plan.validate_states(&steps, Mode::Resume)?;
    if steps.len() >= plan.all().count()
        || plan.all().any(|step| {
            steps
                .get(&step.id)
                .is_some_and(|value| value["artifact_sha256"] != step.artifact)
        })
    {
        return Err("原复制账本已完成或步骤不是原计划绑定的产物".into());
    }
    plan_guard.check()?;
    guard.check()?;
    Ok((head, proof, guard, source))
}

fn fixture_successor(backend: &Path, run: &Path, target: &Target) -> Result<Value> {
    let manifest = common::binding(&run.join("manifest.json"))?;
    services::history::validate_manifest(&common::read_bound(
        backend,
        &manifest,
        "原夹具服务清单",
    )?)?;
    let saved = state::load(run, true)?;
    let history = services::history::validate(run, &saved, true, true)?;
    let restart = original_restart(&history, state::attempts(&saved)?)?;
    let outer = common::read_bound(backend, &restart["result"], "已登记夹具重启")?;
    let generation = common::read_bound(backend, &outer["generation"], "已登记夹具后继代次")?;
    let termination = &generation["predecessor"];
    let predecessor = common::read_bound(backend, termination, "已登记外部终止核对")?;
    let proof = common::read_bound(backend, &predecessor["evidence"], "原 Windows 终止观察")?;
    let selected = &target.storage.value["redis"];
    let initial = &target.initialized["generation"]["storage"]["redis"];
    let previous_runtime = common::read_bound(
        backend,
        &proof["observation"]["evidence"]["redis_runtime"],
        "原 Windows Redis 代次",
    )?;
    let next_request = common::read_bound(
        backend,
        &generation["redis"]["request"],
        "原 Redis 重启请求",
    )?;
    check_redis_successor(
        initial,
        selected,
        &previous_runtime,
        &next_request,
        &generation,
        &proof,
    )?;
    if target.cache.as_ref() != Some(&json!({"redis":selected,"generation":restart["result"]})) {
        return Err("目标 Redis 选择不属于同一原夹具后继".into());
    }
    Ok(
        json!({"manifest":manifest,"restart":restart["result"],"generation":outer["generation"],
        "termination":termination,"termination_evidence":predecessor["evidence"],"redis":selected}),
    )
}

fn original_restart<'a>(history: &Value, attempts: &'a [Value]) -> Result<&'a Value> {
    let mut restarts = attempts.iter().filter(|row| {
        row["stage"] == "fixture-services" && row["mode"] == "restart" && row["status"] == "passed"
    });
    let first = restarts.next().ok_or("原夹具尚未发布成功重启")?;
    if restarts.next().is_some()
        || history["active_generation"] != first["result"]
        || history["closed"] != false
        || !history["external_recovery"].is_null()
        || !history["rustfs_handoff"].is_null()
    {
        return Err("Redis 标记恢复只接受原夹具唯一且当前活跃的重启代次".into());
    }
    Ok(first)
}

fn check_redis_successor(
    initial: &Value,
    selected: &Value,
    previous: &Value,
    request: &Value,
    generation: &Value,
    proof: &Value,
) -> Result<()> {
    let mut allowed = initial.clone();
    for name in ["pid", "started", "run_id"] {
        allowed[name] = selected[name].clone();
    }
    if initial["platform"] != "windows"
        || selected["platform"] != "windows"
        || *selected != allowed
        || initial["run_id"] == selected["run_id"]
        || initial["started"] == selected["started"]
        || previous["redis"] != *initial
        || previous["format_version"] != 3
        || request["format_version"] != 3
        || request["previous_identity"] != previous["process"]
        || request["previous_run_id"] != initial["run_id"]
        || generation["previous_generation"]["kind"] != "initial"
        || generation["redis"]["runtime"]["redis"] != *selected
        || proof["format_version"] != 2
        || proof["kind"] != "reference-fixture-external-termination"
        || proof["observation"]["evidence"]["redis_external"]["kind"]
            != "windows-redis-external-termination"
    {
        return Err("原 Windows Redis 外部终止或同端点后继不完整".into());
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

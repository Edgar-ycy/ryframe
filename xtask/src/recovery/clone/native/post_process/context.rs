//! 复制后与 seed 共享同一持锁事实窗口，沿用原复制和来源证据。
use super::diagnostic::IdentityVerifyCheckpoint;
#[cfg(windows)]
use crate::recovery::clone::native::target_verify::resume_baseline::HeldPrepareHistory;
use crate::recovery::fixture_control::source_pair::execution::HeldTargetSource;
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        cache, copy_ledger, export,
        held_plan::HeldPlanObjects,
        json as document,
        lifecycle::{identity, stage::StageRequest, state},
        resources::common,
        seed,
        target_verify::{
            self,
            guard::{Generation, Inputs},
        },
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct Context<'a> {
    pub backend: &'a Path,
    pub run: &'a Path,
    pub number: u64,
    pub stage: &'a str,
    pub mode: &'a str,
    pub owner: &'a Generation,
    pub scope: &'a Scope,
    pub storage: &'a Path,
    pub manifest: Value,
    pub post: seed::history::Post,
    pub active: Option<seed::history::Post>,
    pub initial: Value,
    pub target: Value,
    pub target_descriptor: Value,
    pub selected: Value,
    pub plan: Value,
    pub copy: PathBuf,
    pub session: Value,
    pub ledger: Value,
    pub source: export::input::Context,
    pub source_generation: Value,
    pub output: PathBuf,
    state: Value,
    protected: Inputs,
    runtime_tree: Option<Value>,
    held_plan: Option<HeldPlanObjects>,
    #[cfg(windows)]
    held_target: Option<&'a HeldTargetSource>,
    #[cfg(windows)]
    held_source_proofs: Option<super::source_proofs::HeldSourceProofs>,
    #[cfg(windows)]
    held_prepare_history: Option<HeldPrepareHistory>,
}

impl<'a> Context<'a> {
    pub(super) fn load(
        request: &StageRequest<'a>,
        owner: &'a Generation,
        scope: &'a Scope,
        storage: &'a Path,
        input: Option<&Value>,
    ) -> Result<Self> {
        Self::load_with_held_target(request, owner, scope, storage, input, None, false)
    }

    pub(super) fn load_with_held_target(
        request: &StageRequest<'a>,
        owner: &'a Generation,
        scope: &'a Scope,
        storage: &'a Path,
        input: Option<&Value>,
        held_target: Option<&'a HeldTargetSource>,
        hold_originals: bool,
    ) -> Result<Self> {
        #[cfg(not(windows))]
        let _ = held_target;
        let backend = request.backend;
        let run = request.run_dir;
        let number = request.attempt.ok_or("复制后阶段序号缺失")?;
        let held = hold_originals.then_some(storage);
        let (manifest, saved, registered) = stage_binding(request, owner, number, input, held)?;
        let (copy, session, ledger) = IdentityVerifyCheckpoint::CompletedCopy
            .for_stage(request, completed_copy(backend, &manifest))?;
        let output = IdentityVerifyCheckpoint::StageOutput
            .for_stage(request, super::files::stage_output(request, number))?;
        let source = export::input::Context::for_copy(backend, run, &output)?;
        let (source_generation, proof_files) =
            source_proof(request.stage, &source, scope, &session["source_export"])?;
        #[cfg(windows)]
        let (proof_files, held_source_proofs) = super::source_proofs::for_stage(
            request,
            &manifest["copy_stage"],
            &session["source_export"],
            proof_files,
            hold_originals,
        )?;
        let runtime_tree = (request.mode == "amend")
            .then(|| target_verify::tree::snapshot(backend, &registered.post.runtime, true))
            .transpose()?;
        let protected = super::files::protected(
            request,
            &copy,
            &registered.post,
            &proof_files,
            &registered.target,
            &source.request,
            &manifest,
        )?;
        let result = Self {
            backend,
            run,
            number,
            stage: request.stage,
            mode: request.mode,
            owner,
            scope,
            storage,
            manifest,
            post: registered.post,
            active: registered.active,
            initial: registered.initial,
            target: registered.target,
            target_descriptor: registered.target_descriptor,
            selected: registered.selected,
            plan: registered.plan,
            copy,
            session,
            ledger,
            source,
            source_generation,
            output,
            state: saved,
            protected,
            runtime_tree,
            held_plan: None,
            #[cfg(windows)]
            held_target,
            #[cfg(windows)]
            held_source_proofs,
            #[cfg(windows)]
            held_prepare_history: registered.held_prepare_history,
        };
        result.check()?;
        Ok(result)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.check_protected()?;
        if let Some(tree) = &self.runtime_tree
            && target_verify::tree::snapshot(self.backend, &self.post.runtime, true)? != *tree
        {
            return Err("环境修订期间原完整运行树或角色归档变化，保留现场".into());
        }
        self.owner.check()?;
        self.scope.assert_unchanged()?;
        if state::load(self.run, true)? != self.state
            || document::read(&self.run.join("manifest.json"))? != self.manifest
            || inspect(self.backend, &self.copy, &self.session)? != self.ledger
        {
            return Err("复制后完整阶段、清单或账本在持锁期间变化".into());
        }
        controller(
            self.backend,
            self.run,
            self.number,
            &self.state,
            self.stage,
            self.mode,
        )?;
        let initialized =
            common::bound_file(self.backend, &self.manifest["initialized"], "初始化后像")?;
        if self.target_history(&initialized)? != (self.initial.clone(), self.target.clone())
            || seed::source_registration::copied::require_with_held_objects(
                self.backend,
                self.run,
                &self.manifest,
                self.held_plan.as_ref(),
            )? != self.plan
        {
            return Err("原初始化、完整计划或复制结果漂移".into());
        }
        if let Some(active) = &self.active {
            let now = seed::history::post(self.backend, self.run, None, Some(self.number), false)?;
            if now.descriptor != active.descriptor
                || now.request != active.request
                || now.runtime != active.runtime
            {
                return Err("活动登记代次变化，禁止回退".into());
            }
        }
        cache::handoff::registration::validate(
            self.backend,
            self.run,
            &self.manifest,
            &self.post,
            &self.plan,
            &self.initial,
        )?;
        let build_path = common::bound_file(
            self.backend,
            &self.post.request["backend_build"],
            "复制后产品构建",
        )?;
        let build = document::read(&build_path)?;
        let product = self.product_root_for(&build_path, &build)?;
        self.scope.product(
            &product,
            &build_path,
            &build,
            document::text(
                &self.source.request["worktree_fingerprint"],
                "原来源完整指纹",
            )?,
        )?;
        quiet(self.backend, self.run, self.number, self.mode, &self.state)?;
        self.check_protected()?;
        self.owner.check()
    }

    fn target_history(&self, initialized: &Path) -> Result<(Value, Value)> {
        #[cfg(windows)]
        if self.held_source_proofs.is_some() {
            return target_verify::published::owned_prepare_with_held_originals(
                self.backend,
                initialized,
                self.owner,
                self.storage,
                self.held_prepare_history.as_ref(),
            );
        }
        target_verify::published::owned_history(self.backend, initialized, self.owner)
    }

    fn check_protected(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(held) = &self.held_source_proofs {
            if self.mode == "prepare" {
                self.protected.check_identities()?;
            } else {
                self.protected.check()?;
            }
            return held.checkpoint();
        }
        self.protected.check()
    }

    #[cfg(windows)]
    pub(super) fn hold_plan(&mut self) -> Result<()> {
        if !super::source_proofs::supports(self.stage, self.mode, &self.manifest["copy_stage"])
            || self.held_plan.is_some()
        {
            return Err("对象原件持有须在同一受支持的 source_to_seed 阶段建立一次".into());
        }
        let path = self.copy.join("plan.json");
        let wrapper = document::read(&path)?;
        if wrapper["plan"] != self.plan {
            return Err("持有对象前复制计划与本阶段检查结果不同".into());
        }
        self.held_plan = Some(HeldPlanObjects::open(self.backend, &path, &wrapper)?);
        self.check()
    }

    pub(super) fn check_end(&self) -> Result<()> {
        let checked = self.check();
        #[cfg(windows)]
        let checked = if self.mode == "prepare" && self.held_source_proofs.is_some() {
            super::files::combine(checked, self.protected.check())
        } else {
            checked
        };
        #[cfg(windows)]
        let checked = super::files::combine(
            checked,
            self.held_prepare_history
                .as_ref()
                .map_or(Ok(()), HeldPrepareHistory::check_end),
        );
        checked?;
        #[cfg(windows)]
        if let Some(held) = &self.held_plan {
            self.scope.assert_unchanged_full()?;
            held.check_end()?;
        }
        #[cfg(windows)]
        if let Some(held) = &self.held_source_proofs {
            held.check_end()?;
        }
        Ok(())
    }

    pub(super) fn product_root(&self) -> Result<PathBuf> {
        let path = common::bound_file(
            self.backend,
            &self.post.request["backend_build"],
            "产品构建",
        )?;
        let build = document::read(&path)?;
        self.product_root_for(&path, &build)
    }

    fn product_root_for(&self, build_path: &Path, build: &Value) -> Result<PathBuf> {
        #[cfg(windows)]
        if let Some(held) = self.held_target {
            return super::runtime::selected_product_root_held(
                self.backend,
                &self.target,
                self.scope,
                build_path,
                build,
                held,
            );
        }
        super::runtime::selected_product_root(
            self.backend,
            &self.target,
            self.scope,
            build_path,
            build,
        )
    }
    pub(super) fn environment(&self) -> Result<BTreeMap<String, String>> {
        let private = common::read_bound(
            self.backend,
            &self.post.request["api_environment"],
            "冻结API环境",
        )?;
        Ok(serde_json::from_value(private["environment"].clone())
            .map_err(|_| "冻结API环境不是文本映射")?)
    }

    pub(super) fn source_generation(&self) -> Result<()> {
        if export::proof::generation_in(&self.source, self.scope)? != self.source_generation {
            return Err("停止的原来源代次不同于复制来源".into());
        }
        Ok(())
    }

    pub(super) fn register(&self) -> Result<Value> {
        if state::attempts(&self.state)?.iter().any(|row| {
            row["stage"] == "post-copy" && row["mode"] == "register" && row["status"] == "passed"
        }) {
            return Err("原始登记已有唯一成功结果；继续prepare，不重复发布原始登记".into());
        }
        self.check()?;
        self.source_generation()?;
        super::files::publish(
            self.backend,
            &self.run.join("post-copy.json"),
            &self.post.request,
        )?;
        self.check()?;
        self.source_generation()?;
        Ok(
            json!({"status":"post_copy_registered","registration":common::binding(&self.run.join("post-copy.json"))?,
            "remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false}),
        )
    }
}

fn completed_copy(backend: &Path, manifest: &Value) -> Result<(PathBuf, Value, Value)> {
    let copy = common::local_directory(
        backend,
        Path::new(document::text(&manifest["copy_directory"], "复制目录")?),
        "复制目录",
    )?;
    let session = document::read(&copy.join("session.json"))?;
    let ledger = inspect(backend, &copy, &session)?;
    Ok((copy, session, ledger))
}

fn inspect(backend: &Path, copy: &Path, session: &Value) -> Result<Value> {
    copy_ledger::inspect(
        backend,
        &copy.join("ledger"),
        document::text(&session["plan_sha256"], "复制计划摘要")?,
        document::text(&session["generation_sha256"], "复制代次摘要")?,
    )
}

fn source_proof(
    stage: &str,
    source: &export::input::Context,
    scope: &Scope,
    binding: &Value,
) -> Result<(Value, Value)> {
    if stage == "seed-runtime" {
        // seed 的目标变更只消费冻结原导出，不要求原源的 API/Worker 仍处于停机窗口。
        let saved = export::frozen::verify(&source.backend, binding)?;
        if saved.request != source.request {
            return Err("seed 的原导出不属于本次复制的固定来源请求".into());
        }
        saved.check_proofs(&source.backend, binding)?;
        return Ok((saved.generation, saved.proof_files));
    }
    let proof = source.verify_copy_export(binding)?;
    let observed = export::proof::generation_in(source, scope)?;
    if observed != proof["generation"] {
        return Err("原来源代次不同于完整复制来源".into());
    }
    Ok((observed, proof["proof_files"].clone()))
}

fn stage_binding(
    request: &StageRequest<'_>,
    owner: &Generation,
    number: u64,
    input: Option<&Value>,
    held_storage: Option<&Path>,
) -> Result<(Value, Value, Registered)> {
    let (manifest, saved) =
        IdentityVerifyCheckpoint::StageState.for_stage(request, stage_state(request, number))?;
    let registered = IdentityVerifyCheckpoint::RegisteredContext.for_stage(
        request,
        registered(request, owner, &manifest, input, held_storage),
    )?;
    let backend = request.backend;
    let run = request.run_dir;
    let quiet_result = quiet(backend, run, number, request.mode, &saved);
    IdentityVerifyCheckpoint::ProducerQuiet.for_stage(request, quiet_result)?;
    Ok((manifest, saved, registered))
}

fn stage_state(request: &StageRequest<'_>, number: u64) -> Result<(Value, Value)> {
    let manifest = document::read(&request.run_dir.join("manifest.json"))?;
    let saved = state::load(request.run_dir, true)?;
    controller(
        request.backend,
        request.run_dir,
        number,
        &saved,
        request.stage,
        request.mode,
    )?;
    Ok((manifest, saved))
}

fn controller(
    backend: &Path,
    run: &Path,
    number: u64,
    saved: &Value,
    stage: &str,
    mode: &str,
) -> Result<()> {
    if !matches!(stage, "post-copy" | "seed-runtime") {
        return Err("未知受控复制后或 seed 阶段".into());
    }
    let row = state::attempts(saved)?.last().ok_or("当前复制后阶段缺失")?;
    if row["number"].as_u64() != Some(number)
        || row["stage"] != stage
        || row["mode"] != mode
        || row["status"] != "running"
    {
        return Err("复制后请求不属于当前唯一正式阶段".into());
    }
    let (_, owner) = state::controller_record(run, number, row)?;
    let current = common::read_bound(
        backend,
        &common::binding(&run.join("run.lock/owner.json"))?,
        "当前运行锁",
    )?;
    if owner != current
        || owner["identity"] != identity::current()?
        || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
    {
        return Err("复制后固定请求必须由当前原生控制器持锁执行".into());
    }
    Ok(())
}

fn quiet(backend: &Path, run: &Path, current: u64, mode: &str, saved: &Value) -> Result<()> {
    if mode != "register" {
        return super::require_quiet(backend, run, Some(current));
    }
    // 尚未发布原始登记时，只有明确未发起生产者的登记尝试可以继续；未知输出仍失败关闭。
    for row in state::attempts(saved)? {
        if row["number"].as_u64() == Some(current) {
            continue;
        }
        match row["stage"].as_str() {
            Some("post-copy") if row["mode"] == "register" => {
                let number = row["number"].as_u64().ok_or("历史登记序号无效")?;
                super::inspection::require_unlaunched(backend, run, "post-copy", number)?;
            }
            Some("post-copy" | "seed-runtime") => {
                return Err("首次登记不能采纳已有业务生产者或seed动作".into());
            }
            _ => (),
        }
    }
    Ok(())
}

struct Registered {
    post: seed::history::Post,
    active: Option<seed::history::Post>,
    initial: Value,
    target: Value,
    target_descriptor: Value,
    selected: Value,
    plan: Value,
    #[cfg(windows)]
    held_prepare_history: Option<HeldPrepareHistory>,
}

fn registered(
    request: &StageRequest<'_>,
    owner: &Generation,
    manifest: &Value,
    input: Option<&Value>,
    held_storage: Option<&Path>,
) -> Result<Registered> {
    let backend = request.backend;
    let run = request.run_dir;
    let number = request.attempt.ok_or("复制后阶段序号缺失")?;
    let active = if request.mode == "register" {
        None
    } else {
        Some(seed::history::post(
            backend,
            run,
            None,
            Some(number),
            false,
        )?)
    };
    let post = match input {
        Some(binding) => seed::history::Post {
            request: common::read_bound(backend, binding, "显式固定请求")?,
            descriptor: binding.clone(),
            runtime: active
                .as_ref()
                .map(|v| v.runtime.clone())
                .unwrap_or_else(|| run.join("post-copy").join("runtime")),
        },
        None => {
            let active = active.as_ref().ok_or("准备缺少活动登记")?;
            seed::history::Post {
                request: active.request.clone(),
                descriptor: active.descriptor.clone(),
                runtime: active.runtime.clone(),
            }
        }
    };
    let initialized = common::bound_file(backend, &manifest["initialized"], "复制后初始化")?;
    #[cfg(windows)]
    let ((initial, target), held_prepare_history) = match held_storage {
        Some(storage)
            if super::source_proofs::supports(
                request.stage,
                request.mode,
                &manifest["copy_stage"],
            ) =>
        {
            target_verify::published::open_prepare_history(backend, &initialized, owner, storage)?
        }
        _ => (
            target_verify::published::owned_history(backend, &initialized, owner)?,
            None,
        ),
    };
    #[cfg(not(windows))]
    let (initial, target) = {
        let _ = held_storage;
        target_verify::published::owned_history(backend, &initialized, owner)?
    };
    let target_descriptor = document::read(
        &initialized
            .parent()
            .ok_or("初始化目录缺失")?
            .join("prepare.json"),
    )?["request"]
        .clone();
    let selected = target_verify::published::request_binding(backend, &target)?["selected"].clone();
    let plan = seed::source_registration::copied::require(backend, run, manifest)?;
    cache::handoff::registration::validate(backend, run, manifest, &post, &plan, &initial)?;
    Ok(Registered {
        post,
        active,
        initial,
        target,
        target_descriptor,
        selected,
        plan,
        #[cfg(windows)]
        held_prepare_history,
    })
}

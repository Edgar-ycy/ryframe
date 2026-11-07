//! 三模式共享同一持锁事实窗口；登记输入不是新的配置或来源权威。
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        cache, copy_ledger, export, json as document,
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
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) struct Context<'a> {
    pub backend: &'a Path,
    pub run: &'a Path,
    pub number: u64,
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
}

impl<'a> Context<'a> {
    pub(super) fn load(
        request: &StageRequest<'a>,
        owner: &'a Generation,
        scope: &'a Scope,
        storage: &'a Path,
        input: Option<&Value>,
    ) -> Result<Self> {
        let backend = request.backend;
        let run = request.run_dir;
        let number = request.attempt.ok_or("复制后阶段序号缺失")?;
        let manifest = document::read(&run.join("manifest.json"))?;
        let saved = state::load(run, true)?;
        controller(backend, run, number, &saved, request.mode)?;
        let Registered {
            post,
            active,
            initial,
            target,
            target_descriptor,
            selected,
            plan,
        } = registered(request, owner, &manifest, input)?;
        quiet(backend, run, number, request.mode, &saved)?;
        let copy = common::local_directory(
            backend,
            Path::new(document::text(&manifest["copy_directory"], "复制目录")?),
            "复制目录",
        )?;
        let session = document::read(&copy.join("session.json"))?;
        let ledger = inspect(backend, &copy, &session)?;
        let base = run.join("post-copy");
        if !base.try_exists()? {
            std::fs::create_dir(&base)?;
        }
        common::local_directory(backend, &base, "复制后证据根")?;
        let output = base.join(format!("attempt-{number:04}"));
        common::ensure_new_directory(backend, &output, "复制后只读核验目录")?;
        let source = export::input::Context::for_copy(backend, run, &output)?;
        let proof = source.verify_copy_export(&session["source_export"])?;
        let source_generation = export::proof::generation_in(&source, scope)?;
        if source_generation != proof["generation"] {
            return Err("原来源代次不同于完整复制来源".into());
        }
        let runtime_tree = if request.mode == "amend" {
            Some(target_verify::tree::snapshot(backend, &post.runtime, true)?)
        } else {
            None
        };
        let protected = protected(
            request,
            &copy,
            &post,
            &proof,
            &target,
            &source.request,
            &manifest,
        )?;
        let result = Self {
            backend,
            run,
            number,
            mode: request.mode,
            owner,
            scope,
            storage,
            manifest,
            post,
            active,
            initial,
            target,
            target_descriptor,
            selected,
            plan,
            copy,
            session,
            ledger,
            source,
            source_generation,
            output,
            state: saved,
            protected,
            runtime_tree,
        };
        result.check()?;
        Ok(result)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.protected.check()?;
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
        controller(self.backend, self.run, self.number, &self.state, self.mode)?;
        let initialized =
            common::bound_file(self.backend, &self.manifest["initialized"], "初始化后像")?;
        if target_verify::published::owned_history(self.backend, &initialized, self.owner)?
            != (self.initial.clone(), self.target.clone())
            || seed::source_registration::copied::require(self.backend, self.run, &self.manifest)?
                != self.plan
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
        let product = super::runtime::selected_product_root(
            self.backend,
            &self.target,
            self.scope,
            &build_path,
            &build,
        )?;
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
        self.protected.check()?;
        self.owner.check()
    }

    pub(super) fn product_root(&self) -> Result<PathBuf> {
        let path = common::bound_file(
            self.backend,
            &self.post.request["backend_build"],
            "产品构建",
        )?;
        let build = document::read(&path)?;
        super::runtime::selected_product_root(self.backend, &self.target, self.scope, &path, &build)
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

fn inspect(backend: &Path, copy: &Path, session: &Value) -> Result<Value> {
    copy_ledger::inspect(
        backend,
        &copy.join("ledger"),
        document::text(&session["plan_sha256"], "复制计划摘要")?,
        document::text(&session["generation_sha256"], "复制代次摘要")?,
    )
}

fn controller(backend: &Path, run: &Path, number: u64, saved: &Value, mode: &str) -> Result<()> {
    let row = state::attempts(saved)?.last().ok_or("当前复制后阶段缺失")?;
    if row["number"].as_u64() != Some(number)
        || row["stage"] != "post-copy"
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
}

fn registered(
    request: &StageRequest<'_>,
    owner: &Generation,
    manifest: &Value,
    input: Option<&Value>,
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
    let (initial, target) = target_verify::published::owned_history(backend, &initialized, owner)?;
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
    })
}

fn protected(
    request: &StageRequest<'_>,
    copy: &Path,
    post: &seed::history::Post,
    proof: &Value,
    target: &Value,
    source: &Value,
    manifest: &Value,
) -> Result<Inputs> {
    let backend = request.backend;
    let run = request.run_dir;
    let number = request.attempt.ok_or("固定输入缺少阶段序号")?;
    let initialized = common::bound_file(backend, &manifest["initialized"], "初始化后像")?;
    let mut paths = BTreeSet::new();
    for path in [
        run.join("manifest.json"),
        run.join("state.json"),
        run.join("run.lock/owner.json"),
        state::controller_path(run, number),
        copy.join("plan.json"),
        copy.join("input.json"),
        copy.join("session.json"),
        copy.join("result.json"),
        copy.join("ledger/head.json"),
    ] {
        paths.insert(common::common_file(backend, &path, "复制后固定文件")?);
    }
    for value in [&post.request, &post.descriptor, target, source, manifest] {
        super::files::bindings(backend, value, &mut paths)?;
    }
    for (path, _) in document::object(&proof["proof_files"], "原来源完整证明文件")? {
        paths.insert(common::common_file(
            backend,
            Path::new(path),
            "原来源证明文件",
        )?);
    }
    paths.extend(target_verify::tree::files(
        initialized.parent().ok_or("初始化目录缺失")?,
        &target_verify::tree::snapshot(
            backend,
            initialized.parent().ok_or("初始化目录缺失")?,
            true,
        )?,
    )?);
    for request in [&post.request, target, source] {
        for field in ["backend_build", "maintenance_build"] {
            if request[field].is_null() {
                continue;
            }
            let build = common::read_bound(backend, &request[field], "冻结构建")?;
            for artifact in document::object(&build["artifacts"], "冻结构建产物")?.values() {
                paths.insert(target_verify::input::request::external_file(
                    &json!({"path":artifact["executable"],"sha256":artifact["sha256"]}),
                )?);
            }
        }
    }
    if request.mode == "amend" {
        let tree = target_verify::tree::snapshot(backend, &post.runtime, true)?;
        paths.extend(
            target_verify::tree::files(&post.runtime, &tree)?
                .into_iter()
                .filter(|path| {
                    !matches!(
                        path.file_name().and_then(|v| v.to_str()),
                        Some(
                            "api-control.guard" | "worker-control.guard" | "runtime-control.guard"
                        )
                    )
                }),
        );
        let runtime = document::read(&post.runtime.join("runtime.json"))?;
        for field in ["api_config_dir", "worker_config_dir"] {
            let directory = common::local_directory(
                backend,
                Path::new(document::text(&runtime[field], "原私有角色配置目录")?),
                "原私有角色配置目录",
            )?;
            for name in ["app.toml", "app.test.toml"] {
                let path = directory.join(name);
                if path.try_exists()? {
                    paths.insert(common::common_file(backend, &path, "原私有角色配置")?);
                }
            }
        }
    }
    Inputs::open(paths)
}

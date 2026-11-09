//! API 交接冻结原登记、完整复制计划/账本、来源与文件，不重新签发历史。
use crate::{
    Result,
    recovery::clone::native::{
        self,
        bridge::scope::Scope,
        copy_ledger, export, json as document,
        lifecycle::{identity, state},
        resources::common,
        seed,
        target_verify::{
            self,
            guard::{Generation, Inputs},
        },
    },
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) struct Frame {
    pub backend: PathBuf,
    pub product: PathBuf,
    pub run: PathBuf,
    pub number: u64,
    pub manifest: Value,
    pub post: seed::history::Post,
    pub initial: Value,
    pub target: Value,
    pub target_descriptor: Value,
    pub selected: Value,
    pub source: export::input::Context,
    pub source_generation: Value,
    pub plan: Value,
    pub copy: PathBuf,
    pub ledger: Value,
    pub output: PathBuf,
    pub runtime: Value,
    pub state: Value,
    pub controller: Value,
    pub owner: Value,
    pub protected: Inputs,
}

impl Frame {
    pub(super) fn load(
        backend: &Path,
        run: &Path,
        number: u64,
        owner: &Generation,
        scope: &Scope,
    ) -> Result<Self> {
        let manifest = document::read(&run.join("manifest.json"))?;
        let post = seed::history::post(backend, run, None, None, false)?;
        let initialized = common::bound_file(backend, &manifest["initialized"], "API交接初始化")?;
        let (initial, target) =
            target_verify::published::owned_history(backend, &initialized, owner)?;
        let prepared = document::read(
            &initialized
                .parent()
                .ok_or("初始化目录缺失")?
                .join("prepare.json"),
        )?;
        let target_descriptor = prepared["request"].clone();
        let selected =
            target_verify::published::request_binding(backend, &target)?["selected"].clone();
        let plan = seed::source_registration::copied::require(backend, run, &manifest)?;
        super::registration::validate(backend, run, &manifest, &post, &plan, &initial)?;
        let copy = common::local_directory(
            backend,
            Path::new(document::text(&manifest["copy_directory"], "复制目录")?),
            "复制目录",
        )?;
        let session = document::read(&copy.join("session.json"))?;
        let ledger = copy_ledger::inspect(
            backend,
            &copy.join("ledger"),
            document::text(&session["plan_sha256"], "复制计划摘要")?,
            document::text(&session["generation_sha256"], "复制代次摘要")?,
        )?;
        let output = run.join(format!("post-copy/attempt-{number:04}"));
        common::ensure_new_directory(backend, &output, "API交接核验目录")?;
        let source = export::input::Context::for_copy(backend, run, &output)?;
        let source_proof = source.verify_copy_export(&session["source_export"])?;
        let source_generation = export::proof::generation_in(&source, scope)?;
        if source_generation != source_proof["generation"] {
            return Err("API交接来源代次不同于原复制来源".into());
        }
        let runtime = super::registration::runtime(
            backend,
            run,
            &post,
            &selected["api_url"],
            &target,
            scope,
        )?;
        let product = native::post_process::runtime::product_root(backend, &runtime)?;
        let state = state::load(run, true)?;
        let (controller, run_owner) = controller(backend, run, number, &state)?;
        let mut paths = files(
            backend,
            run,
            &copy,
            &post,
            &source_proof,
            &target,
            &source.request,
        )?;
        paths.extend(target_verify::tree::files(
            initialized.parent().ok_or("初始化目录缺失")?,
            &target_verify::tree::snapshot(
                backend,
                initialized.parent().ok_or("初始化目录缺失")?,
                true,
            )?,
        )?);
        let protected = Inputs::open(paths)?;
        let frame = Self {
            backend: backend.to_owned(),
            product,
            run: run.to_owned(),
            number,
            manifest,
            post,
            initial,
            target,
            target_descriptor,
            selected,
            source,
            source_generation,
            plan,
            copy,
            ledger,
            output,
            runtime,
            state,
            controller,
            owner: run_owner,
            protected,
        };
        frame.check(owner, scope)?;
        Ok(frame)
    }

    pub(super) fn check(&self, owner: &Generation, scope: &Scope) -> Result<()> {
        self.protected.check()?;
        owner.check()?;
        scope.assert_unchanged()?;
        if state::load(&self.run, true)? != self.state
            || controller(&self.backend, &self.run, self.number, &self.state)?
                != (self.controller.clone(), self.owner.clone())
            || document::read(&self.run.join("manifest.json"))? != self.manifest
        {
            return Err("API交接控制器、完整账本或固定清单变化".into());
        }
        let post = seed::history::post(&self.backend, &self.run, None, None, false)?;
        if post.request != self.post.request
            || post.descriptor != self.post.descriptor
            || post.runtime != self.post.runtime
        {
            return Err("API交接活动登记发生变化，不能回退旧成功".into());
        }
        let filename = common::bound_file(
            &self.backend,
            &self.manifest["initialized"],
            "API交接初始化后像",
        )?;
        if target_verify::published::owned_history(&self.backend, &filename, owner)?
            != (self.initial.clone(), self.target.clone())
            || seed::source_registration::copied::require(&self.backend, &self.run, &self.manifest)?
                != self.plan
        {
            return Err("API交接初始化、完整计划或复制后像变化".into());
        }
        let session = document::read(&self.copy.join("session.json"))?;
        if copy_ledger::inspect(
            &self.backend,
            &self.copy.join("ledger"),
            document::text(&session["plan_sha256"], "复制计划摘要")?,
            document::text(&session["generation_sha256"], "复制代次摘要")?,
        )? != self.ledger
        {
            return Err("API交接完整复制账本变化".into());
        }
        super::registration::validate(
            &self.backend,
            &self.run,
            &self.manifest,
            &self.post,
            &self.plan,
            &self.initial,
        )?;
        if super::registration::runtime(
            &self.backend,
            &self.run,
            &self.post,
            &self.selected["api_url"],
            &self.target,
            scope,
        )? != self.runtime
        {
            return Err("API交接运行合同或配置在保护期间变化".into());
        }
        native::post_process::require_quiet(&self.backend, &self.run, Some(self.number))?;
        self.protected.check()?;
        owner.check()?;
        scope.assert_unchanged()
    }
}

fn controller(backend: &Path, run: &Path, number: u64, saved: &Value) -> Result<(Value, Value)> {
    let attempt = state::attempts(saved)?
        .last()
        .ok_or("API交接缺少当前阶段")?;
    if attempt["number"].as_u64() != Some(number)
        || attempt["stage"] != "runtime-target"
        || attempt["mode"] != "start"
        || attempt["status"] != "running"
    {
        return Err("API交接必须属于当前唯一runtime-target/start阶段".into());
    }
    let (binding, owner) = state::controller_record(run, number, attempt)?;
    let current = common::read_bound(
        backend,
        &common::binding(&run.join("run.lock/owner.json"))?,
        "API交接运行锁",
    )?;
    if owner != current
        || owner["identity"] != identity::current()?
        || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
    {
        return Err("API交接必须由原生当前控制器持有原运行锁".into());
    }
    Ok((binding, owner))
}

fn files(
    backend: &Path,
    run: &Path,
    copy: &Path,
    post: &seed::history::Post,
    export: &Value,
    target: &Value,
    source: &Value,
) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for path in [
        run.join("manifest.json"),
        run.join("state.json"),
        run.join("run.lock/owner.json"),
        copy.join("plan.json"),
        copy.join("input.json"),
        copy.join("session.json"),
        copy.join("result.json"),
        copy.join("ledger/head.json"),
    ] {
        paths.insert(common::common_file(backend, &path, "API交接固定文件")?);
    }
    for value in [&post.request, &post.descriptor, target, source] {
        bindings(backend, value, &mut paths)?;
    }
    for request in [target, source] {
        for field in ["backend_build", "maintenance_build"] {
            if request[field].is_null() {
                continue;
            }
            let build = common::read_bound(backend, &request[field], "API交接冻结构建")?;
            for artifact in document::object(&build["artifacts"], "API交接构建产物")?.values()
            {
                paths.insert(target_verify::input::request::external_file(
                    &serde_json::json!({
                    "path":artifact["executable"],"sha256":artifact["sha256"]}),
                )?);
            }
        }
    }
    for name in ["runtime.json", "binaries.json", "handoff.json"] {
        paths.insert(common::common_file(
            backend,
            &post.runtime.join(name),
            "API交接冻结运行文件",
        )?);
    }
    for (path, _) in document::object(&export["proof_files"], "完整来源证据文件")? {
        paths.insert(common::common_file(
            backend,
            Path::new(path),
            "原来源证据文件",
        )?);
    }
    for role in ["api", "worker"] {
        let runtime = document::read(&post.runtime.join("runtime.json"))?;
        paths.insert(target_verify::input::request::external_file(
            &runtime["artifacts"][role],
        )?);
    }
    for field in ["api_config_dir", "worker_config_dir"] {
        let runtime = document::read(&post.runtime.join("runtime.json"))?;
        let directory = Path::new(document::text(&runtime[field], "API交接配置目录")?);
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) == Some("toml") {
                paths.insert(common::common_file(backend, &path, "API交接绑定配置")?);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub(super) fn bindings(backend: &Path, value: &Value, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
    match value {
        Value::Object(map)
            if map.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|k| map.contains_key(*k)) =>
        {
            paths.insert(common::bound_file(backend, value, "API交接文件绑定")?);
        }
        Value::Object(map)
            if map.len() == 2 && ["path", "sha256"].iter().all(|k| map.contains_key(*k)) =>
        {
            paths.insert(target_verify::input::request::external_file(value)?);
        }
        Value::Object(map) => {
            for (key, value) in map {
                if key == "execution_backend" && value.get("execution_source").is_some() {
                    paths.extend(
                        crate::recovery::fixture_control::source_pair::execution::target_protected_inputs(
                            backend, value,
                        )?,
                    );
                } else {
                    bindings(backend, value, paths)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                bindings(backend, value, paths)?;
            }
        }
        _ => (),
    }
    Ok(())
}

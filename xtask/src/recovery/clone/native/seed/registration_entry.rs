//! seed 运行登记直接使用同一原生控制器与已收尾复制证据。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::SeedRuntimeOptions,
    recovery::clone::native::{
        bridge::scope::Scope,
        cache, json as document,
        lifecycle::{
            identity, manifest,
            stage::{self, StageRequest, StageRunner},
            state,
        },
        post_process::{self, files, ownership},
        resources::common,
        target_verify::guard::Inputs,
    },
};

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &SeedRuntimeOptions,
) -> Result<Value> {
    let path = options
        .request
        .as_deref()
        .ok_or("seed 登记需要明确 --request")?;
    if options.producer_binding.is_some() {
        return Err("seed 登记只接受明确的登记请求".into());
    }
    let runner = Runner {
        input: common::binding(&common::common_file(backend, path, "seed 登记请求")?)?,
    };
    let value = stage::run_named(
        backend,
        &options.run_dir,
        "seed-runtime",
        "register",
        &runner,
    )?;
    Ok(
        json!({"status":value["status"],"stage":value["stage"],"mode":value["mode"],
        "attempt":value["attempt"],"restore_qualified":value["restore_qualified"]}),
    )
}

struct Runner {
    input: Value,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "seed-runtime" || request.mode != "register" {
            return Err("seed 登记必须使用同一正式运行阶段".into());
        }
        common::bound_file(request.backend, &self.input, "seed 登记请求")?;
        let path = common::common_file(
            request.backend,
            &request.run_dir.join("manifest.json"),
            "seed 运行清单",
        )?;
        environments(request.backend, &document::read(&path)?)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        let number = request.attempt.ok_or("seed 登记缺少实际阶段序号")?;
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let frame = Frame::load(request, number, &self.input, &manifest.value)?;
        let scope = Scope::load(
            request.backend,
            document::array(&manifest.value["build_bridges"], "seed 产品来源桥接")?,
            &cache::runner::inherited_bridges(request.backend, &manifest.value)?,
        )?;
        let storage = cache::handoff::locks::Storage::acquire(
            request.backend,
            request.run_dir,
            &manifest.value,
        )?;
        ownership::with_target(request, &manifest.value, storage, |owner, _| {
            frame.check(request, &self.input)?;
            scope.assert_unchanged()?;
            owner.check()?;
            post_process::require_quiet(request.backend, request.run_dir, Some(number))?;
            let result = super::publish(request.backend, request.run_dir, &frame.path);
            let checked = (|| {
                post_process::require_quiet(request.backend, request.run_dir, Some(number))?;
                frame.check(request, &self.input)?;
                scope.assert_unchanged()?;
                owner.check()
            })();
            files::combine(result, checked)
        })
    }
}

struct Frame {
    path: PathBuf,
    state: Value,
    held: Inputs,
}

impl Frame {
    fn load(
        request: &StageRequest<'_>,
        number: u64,
        input: &Value,
        manifest: &Value,
    ) -> Result<Self> {
        let run = request.run_dir;
        let backend = request.backend;
        let saved = state::load(run, true)?;
        let row = state::running_attempt(&saved)?.ok_or("seed 登记缺少运行中阶段")?;
        let (binding, owner) = state::controller_record(run, number, row)?;
        let lock = run.join("run.lock/owner.json");
        if row["number"].as_u64() != Some(number)
            || row["stage"] != request.stage
            || row["mode"] != request.mode
            || owner != document::read(&lock)?
            || owner["identity"] != identity::current()?
            || owner["directory"] != json!(run)
            || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
        {
            return Err("seed 登记不属于当前持锁运行控制器".into());
        }
        let path = common::bound_file(backend, input, "seed 登记请求")?;
        let value = common::read_bound(backend, input, "seed 登记请求")?;
        let mut paths = BTreeSet::new();
        for file in [
            run.join("state.json"),
            run.join("manifest.json"),
            lock,
            path.clone(),
        ] {
            paths.insert(common::common_file(backend, &file, "seed 登记保护文件")?);
        }
        for value in [manifest, &value, &binding] {
            files::bindings(backend, value, &mut paths)?;
        }
        let held = Inputs::open(paths)?;
        if document::read(&run.join("manifest.json"))? != *manifest
            || document::read(&run.join("run.lock/owner.json"))? != owner
        {
            return Err("seed 登记的清单或控制锁在取得保护前变化".into());
        }
        environments(backend, manifest)?;
        let result = Self {
            path,
            state: saved,
            held,
        };
        result.check(request, input)?;
        Ok(result)
    }

    fn check(&self, request: &StageRequest<'_>, input: &Value) -> Result<()> {
        self.held.check()?;
        if state::load(request.run_dir, true)? != self.state
            || common::bound_file(request.backend, input, "seed 登记原请求")? != self.path
        {
            return Err("seed 登记的阶段记录或请求变化".into());
        }
        self.held.check()
    }
}

fn environments(backend: &Path, manifest: &Value) -> Result<()> {
    for field in ["source_environment", "target_environment"] {
        common::environment(backend, &manifest[field])
            .map_err(|_| format!("seed 运行清单的 {field} 必须绑定唯一 environment 文本映射"))?;
    }
    Ok(())
}

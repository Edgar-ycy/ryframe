//! 复用已核验的双角色运行合同，发布本次 seed 运行交接。

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{files, prepare::require_runtime_receipts, seed_capacity::Guard};
use crate::{
    Result,
    recovery::{
        clone::native::{
            cache::handoff::locks::seed_runtime_guard,
            json as document,
            lifecycle::lock::ProcessGuard,
            resources::common,
            seed::{capacity, runtime_model, source_registration::identity},
            target_verify::guard::Inputs,
        },
        fixture_control::{full_stack_runtime_contract, verify_full_stack_runtime},
    },
};

pub(super) fn execute(
    guard: &Guard<'_, '_>,
    request: &Value,
    registration: &Value,
) -> Result<Value> {
    let context = guard.live.context;
    let backend = context.backend;
    let run = context.run;
    let current = Some(context.number);
    let identity = identity::verified(backend, run, request, current)?;
    let quota = capacity::evidence(backend, run, request, current)?;
    runtime_model::api_environment(backend, &context.manifest, &context.post)?;
    let schedules = guard.full()?;
    let product = context.product_root()?;
    let original = verify_full_stack_runtime(&product, &context.post.runtime)?;
    let runtime = run.join("seed-runtime/runtime");
    if !runtime.try_exists()? {
        fs::create_dir(&runtime)?;
    }
    let lock = seed_runtime_guard(backend, run)?;
    let proof = Prepared {
        identity,
        quota,
        schedules,
        product,
        original,
    };
    let result = publish(guard, request, registration, &runtime, &lock, proof);
    files::combine(result, lock.check())
}

struct Prepared {
    identity: Value,
    quota: Value,
    schedules: Vec<Value>,
    product: PathBuf,
    original: Value,
}

fn publish(
    guard: &Guard<'_, '_>,
    request: &Value,
    registration: &Value,
    runtime: &Path,
    lock: &ProcessGuard,
    proof: Prepared,
) -> Result<Value> {
    let context = guard.live.context;
    let backend = context.backend;
    let run = context.run;
    let current = Some(context.number);
    let Prepared {
        identity: proof,
        quota,
        schedules,
        product,
        original,
    } = proof;
    require_runtime_receipts(backend, runtime)?;
    let binaries = runtime.join("binaries.json");
    let contract = runtime.join("runtime.json");
    let handoff = runtime.join("handoff.json");
    let original_binaries = document::read(&context.post.runtime.join("binaries.json"))?;
    context.check()?;
    lock.check()?;
    files::publish(backend, &binaries, &original_binaries)?;
    let api = Path::new(document::text(
        &original["api_config_dir"],
        "原 API 配置目录",
    )?);
    let worker = Path::new(document::text(
        &original["worker_config_dir"],
        "原 Worker 配置目录",
    )?);
    if full_stack_runtime_contract(&product, runtime, api, worker)? != original {
        return Err("seed 运行配置、scope 或产品产物不同于原合同".into());
    }
    files::publish(backend, &contract, &original)?;
    if verify_full_stack_runtime(&product, runtime)? != original {
        return Err("seed 运行合同发布后变化；保留现场".into());
    }
    let value = json!({"registration":registration,"identity":proof,"capacity":quota,
        "schedules":schedules,"api_environment":context.post.request["api_environment"],
        "backend_build":context.post.request["backend_build"],"original_api":request["api_process"],
        "runtime":common::binding(&contract)?,"binaries":common::binding(&binaries)?,
        "api_url":context.selected["api_url"],"scope_id":context.target["target"]["scope_id"]});
    context.check()?;
    files::publish(backend, &handoff, &value)?;
    let descriptor = common::binding(&handoff)?;
    let protected = Inputs::open([binaries, contract, handoff])?;
    if guard.full()? != schedules
        || identity::verified(backend, run, request, current)? != proof
        || capacity::evidence(backend, run, request, current)? != quota
        || verify_full_stack_runtime(&product, runtime)? != original
    {
        return Err("seed 准备收尾期间身份、容量、调度或运行合同变化；保留现场".into());
    }
    require_runtime_receipts(backend, runtime)?;
    common::bound_file(backend, &value["runtime"], "seed 运行合同")?;
    common::bound_file(backend, &value["binaries"], "seed 二进制清单")?;
    if common::read_bound(backend, &descriptor, "seed 运行交接")? != value {
        return Err("seed 运行交接在取得保护前发生变化".into());
    }
    protected.check()?;
    context.check()?;
    lock.check()?;
    Ok(
        json!({"status":"seed_runtime_prepared","handoff":descriptor,"remote_writes":0,
        "outbox_drained":false,"restore_qualified":false}),
    )
}

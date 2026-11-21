//! 复制与复制后阶段只读复核正式运行已绑定的来源代次。
use serde_json::Value;

use super::super::input::Context;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            bridge::{self, scope::Scope},
            cache, json as document,
            lifecycle::manifest,
            maintenance,
            target_verify::{authority, guard::Inputs},
        },
        fixture_control::source_pair::execution::HeldTargetSource,
        source::evidence::binding,
    },
};

pub(crate) fn request(args: &Value) -> Result<Value> {
    request_in(args, None)
}

pub(in crate::recovery::clone::native) fn request_in(
    args: &Value,
    held: Option<&HeldTargetSource>,
) -> Result<Value> {
    document::exact(args, &["backend", "run_dir"], "复制来源代次参数")?;
    let backend = binding::path(&args["backend"])?;
    let run = binding::path(&args["run_dir"])?;
    validate_local_test_path(&run, &backend, LocalTestPathKind::ExistingDirectory)?;
    let path = run.join("manifest.json");
    let manifest_held = Inputs::open([path.clone()])?;
    let result = (|| {
        let registered = manifest::load(&backend, &path)?;
        let source_request = &registered.value["source_request"];
        let source_environment = &registered.value["source_environment"];
        let request_inputs = Inputs::open([
            manifest::bound_file(&backend, source_request, "复制来源请求", true)?,
            manifest::bound_file(&backend, source_environment, "复制来源环境", true)?,
        ])?;
        let result = (|| {
            let context =
                Context::from_copy_manifest(&backend, &run, &run, registered.value.clone())?;
            observe(&context, held)
        })();
        let checked = (|| {
            request_inputs.check()?;
            manifest::bound_file(&backend, source_request, "复制来源请求", true)?;
            manifest::bound_file(&backend, source_environment, "复制来源环境", true)?;
            Ok(())
        })();
        checked_result(result, checked)
    })();
    checked_result(result, manifest_held.check())
}

fn observe(context: &Context, held: Option<&HeldTargetSource>) -> Result<Value> {
    let bindings = document::array(&context.manifest["build_bridges"], "复制来源构建桥接")?;
    let inherited = cache::runner::inherited_bridges(&context.backend, &context.manifest)?;
    let scope = Scope::load(&context.backend, bindings, &inherited)?;
    observe_in_held(context, &scope, held)
}

pub(crate) fn observe_in(context: &Context, scope: &Scope) -> Result<Value> {
    observe_in_held(context, scope, None)
}

fn observe_in_held(
    context: &Context,
    scope: &Scope,
    held: Option<&HeldTargetSource>,
) -> Result<Value> {
    let jobs = bridge::build_jobs(&context.execution_backend)?;
    let value = super::generation_observed(context, scope, require_closed, true, held)?;
    maintenance::require_build_jobs(&value["maintenance"], jobs)?;
    if bridge::build_jobs(&context.execution_backend)? != jobs {
        return Err("复制来源复核期间有效维护并发配置发生变化".into());
    }
    Ok(value)
}

fn require_closed(api: &str, worker: &str) -> Result<()> {
    authority::require_closed(api)?;
    authority::require_closed(worker)
}

fn checked_result(result: Result<Value>, checked: Result<()>) -> Result<Value> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => {
            Err(format!("复制来源代次核验失败：{error}；原绑定后验失败：{check}").into())
        }
    }
}

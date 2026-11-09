//! 环境修订只发布下一登记代次；原失败、控制结果和历史文件保持原字节。
use super::context::Context;
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document, lifecycle::state, resources::common, seed,
    },
};
use serde_json::{Value, json};
use std::path::Path;
mod proof;

pub(super) fn run(context: &Context<'_>) -> Result<Value> {
    let active = context.active.as_ref().ok_or("环境修订缺少活动登记")?;
    candidate(context.backend, &active.request, &context.post.request)?;
    let saved = state::load(context.run, true)?;
    allowed(context.run, &saved)?;
    let product = context.product_root()?;
    let invalidated = proof::capture(context.backend, &product, context.run, &saved, active)?;
    let mut proof_files = std::collections::BTreeSet::new();
    super::files::bindings(context.backend, &invalidated, &mut proof_files)?;
    let proof_guard =
        crate::recovery::clone::native::target_verify::guard::Inputs::open(proof_files)?;
    let sequence = sequence(context.backend, active)?
        .checked_add(1)
        .filter(|v| *v <= 9999)
        .ok_or("post-copy amendment代次已超出合法范围")?;
    let directory = context.run.join("post-copy").join("amendments");
    if sequence == 1 {
        std::fs::create_dir(&directory)?;
    }
    common::local_directory(context.backend, &directory, "环境修订目录")?;
    let path = directory.join(format!("{sequence:04}.json"));
    super::files::absent(&path)?;
    let payload = json!({"format_version":1,"kind":"devex-clone-post-copy-amendment","sequence":sequence,
        "run_manifest":state::manifest_binding(context.run)?,"predecessor":active.descriptor,
        "request_sha256":digest::plan_hash(&context.post.request)?,"api_environment":context.post.request["api_environment"],
        "invalidated_runtime":invalidated});
    context.check()?;
    context.source_generation()?;
    if proof::capture(context.backend, &product, context.run, &saved, active)? != invalidated {
        return Err("环境修订前原运行关闭证明变化".into());
    }
    document::write_new(&path, &payload)?;
    let binding = common::binding(&path)?;
    context.check()?;
    context.source_generation()?;
    if proof::capture(context.backend, &product, context.run, &saved, active)? != invalidated {
        return Err("环境修订后原运行关闭证明变化，保留新旧证据，不自动重放".into());
    }
    common::bound_file(context.backend, &binding, "新环境修订后像")?;
    proof_guard.check()?;
    Ok(
        json!({"status":"post_copy_amended","registration":binding,"sequence":sequence,"predecessor":active.descriptor,
        "runtime_directory":context.run.join("post-copy").join(format!("runtime-{sequence:04}")),
        "remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false}),
    )
}

pub(super) fn candidate(backend: &Path, previous: &Value, next: &Value) -> Result<()> {
    let mut expected = previous.clone();
    expected["api_environment"] = next["api_environment"].clone();
    if next != &expected || next["api_environment"] == previous["api_environment"] {
        return Err("环境修订只能明确重新绑定api_environment，不能更改业务或其他请求字段".into());
    }
    let initial = common::read_bound(backend, &previous["api_environment"], "旧API环境")?;
    let api = common::read_bound(backend, &next["api_environment"], "新API环境")?;
    crate::recovery::source::evidence::validate_copy_api_environment(
        &json!({"initial":initial["environment"],"api":api["environment"]}),
    )?;
    Ok(())
}

fn allowed(run: &Path, saved: &Value) -> Result<()> {
    super::files::absent(&run.join("seed-runtime.json"))?;
    if state::attempts(saved)?.iter().any(|row| {
        row["stage"] == "seed-runtime"
            || row["stage"] == "post-copy"
                && matches!(
                    row["mode"].as_str(),
                    Some("schedules" | "reconcile" | "verify")
                )
    }) {
        return Err("业务动作或seed已经开始，不能修订API环境绑定".into());
    }
    Ok(())
}

fn sequence(backend: &Path, active: &seed::history::Post) -> Result<u64> {
    let value = common::read_bound(backend, &active.descriptor, "已验证活动登记")?;
    match value["kind"].as_str() {
        Some("devex-clone-post-copy") => Ok(0),
        Some("devex-clone-post-copy-amendment") => value["sequence"]
            .as_u64()
            .filter(|v| *v > 0 && *v < 9999)
            .ok_or_else(|| "活动修订序号无效".into()),
        _ => Err("活动登记类型无效".into()),
    }
}

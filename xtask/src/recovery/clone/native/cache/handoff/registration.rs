//! 当前 post-copy 请求及 prepare 只消费原已发布链，不回退或补签。
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document, lifecycle::state, resources::common, seed, session, target_verify,
    },
};
use serde_json::{Value, json};
use std::path::Path;

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "run_manifest",
    "copy_stage_receipt",
    "copy_result",
    "ledger_head",
    "api_environment",
    "backend_build",
    "node",
    "source_admin",
    "frontend_root",
    "pacing",
    "reference_plan",
    "dataset",
    "administrator_credentials",
];

pub(crate) fn validate(
    backend: &Path,
    run: &Path,
    manifest: &Value,
    post: &seed::history::Post,
    plan: &Value,
    initial: &Value,
) -> Result<()> {
    let request = &post.request;
    document::exact(request, FIELDS, "复制后固定请求")?;
    if request["format_version"].as_u64() != Some(1)
        || request["kind"] != "devex-clone-post-copy"
        || request["run_manifest"] != state::manifest_binding(run)?
    {
        return Err("复制后登记类型或原运行绑定变化".into());
    }
    let saved = state::load(run, true)?;
    let copied = state::attempts(&saved)?
        .iter()
        .rfind(|row| row["stage"] == "copy")
        .ok_or("API启动缺少原完整复制阶段")?;
    let copy = Path::new(document::text(&manifest["copy_directory"], "复制目录")?);
    if copied["status"] != "passed"
        || request["copy_stage_receipt"] != copied["result"]
        || request["copy_result"] != common::binding(&copy.join("result.json"))?
        || request["ledger_head"] != common::binding(&copy.join("ledger/head.json"))?
    {
        return Err("复制后登记必须绑定最新复制成功阶段及完整账本head".into());
    }
    for field in [
        "run_manifest",
        "copy_stage_receipt",
        "copy_result",
        "ledger_head",
        "api_environment",
        "backend_build",
        "reference_plan",
        "dataset",
        "administrator_credentials",
    ] {
        common::bound_file(backend, &request[field], "复制后登记文件")?;
    }
    target_verify::input::request::external_file(&request["node"])?;
    let build = json!({"path":request["backend_build"]["path"],"sha256":request["backend_build"]["sha256"]});
    let mut accepted = false;
    for bridge in document::array(&manifest["build_bridges"], "本运行构建桥接")? {
        let path = common::common_file(
            backend,
            Path::new(document::text(&bridge["path"], "构建桥接路径")?),
            "构建桥接",
        )?;
        if digest::file(&path)?.sha256 != document::text(&bridge["sha256"], "构建桥接摘要")? {
            return Err("构建桥接文件字节变化".into());
        }
        if document::read(&path)?["build"] == build {
            accepted = true;
        }
    }
    if !accepted {
        return Err("API产物必须属于本run已审计的产品构建".into());
    }
    let old = common::read_bound(backend, &manifest["target_environment"], "原目标环境")?;
    let api = common::read_bound(backend, &request["api_environment"], "API冻结环境")?;
    document::exact(&api, &["environment"], "API冻结环境")?;
    crate::recovery::source::evidence::validate_copy_api_environment(&json!({
        "initial":old["environment"],"api":api["environment"]}))?;
    frontend(&initial["generation"]["selected"]["frontend_url"])?;
    session::credentials::validate_request(
        &json!({"backend":backend,"identity":request["source_admin"],
        "administrator_credentials":request["administrator_credentials"]}),
    )?;
    session::pacing_binding::request(&json!({"backend":backend,"directory":run,
        "frontend_root":request["frontend_root"],"pacing":request["pacing"]}))?;
    lineage(backend, manifest, request, plan)?;
    common::bound_file(backend, &post.descriptor, "复制后活动登记后像")?;
    Ok(())
}

fn frontend(value: &Value) -> Result<()> {
    let url = url::Url::parse(document::text(value, "前端端点")?).map_err(|_| "前端端点无效")?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]"))
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("前端必须使用已审阅的明确本机HTTP端点".into());
    }
    Ok(())
}

fn lineage(backend: &Path, manifest: &Value, request: &Value, plan: &Value) -> Result<()> {
    let reference = common::read_bound(backend, &request["reference_plan"], "原业务计划")?;
    let dataset = common::read_bound(backend, &request["dataset"], "原业务数据集")?;
    let scope = document::text(&reference["source"]["scope_id"], "原业务来源scope")?;
    if dataset["plan_sha256"] != digest::plan_hash(&reference)?
        || dataset["source_scope_id"] != scope
        || plan["copy_stage"] != manifest["copy_stage"]
    {
        return Err("原业务计划、数据集身份或复制阶段不一致".into());
    }
    if plan["copy_stage"] == "source_to_seed" {
        if plan["source_scope"] != scope {
            return Err("既有业务数据不属于复制计划直接来源".into());
        }
        return Ok(());
    }
    if plan["copy_stage"] != "seed_to_arm" {
        return Err("复制后API不支持未知复制阶段".into());
    }
    let inherited = crate::recovery::source::evidence::comparison::copy_source(backend, manifest)?;
    let result = common::read_bound(
        backend,
        &manifest["source_registration"],
        "已发布seed来源结果",
    )?;
    let registration = common::read_bound(backend, &result["registration"], "原seed来源登记")?;
    let origin = common::read_bound(backend, &registration["run_manifest"], "原seed复制清单")?;
    let directory = Path::new(document::text(
        &registration["run_directory"],
        "原seed运行",
    )?);
    let previous = seed::history::post(
        backend,
        directory,
        Some(&registration["post_copy"]),
        None,
        true,
    )?;
    if previous.request["reference_plan"] != request["reference_plan"]
        || previous.request["dataset"] != request["dataset"]
        || inherited["source_request"] != manifest["source_request"]
        || inherited["source_environment"] != manifest["source_environment"]
        || inherited["request"]["source"]["scope_id"] != plan["source_scope"]
    {
        return Err("seed_to_arm必须继承同一已发布源和原post-copy业务绑定".into());
    }
    let original = common::read_bound(backend, &origin["source_request"], "原始业务来源请求")?;
    if original["source"]["scope_id"] != scope {
        return Err("seed_to_arm原始来源scope与数据集不同".into());
    }
    Ok(())
}

pub(super) fn runtime(
    backend: &Path,
    run: &Path,
    post: &seed::history::Post,
    api_url: &Value,
    target: &Value,
    scope: &crate::recovery::clone::native::bridge::scope::Scope,
) -> Result<Value> {
    let runtime = recorded_runtime(backend, run, post, Some(api_url), false)?;
    let path = common::bound_file(backend, &post.request["backend_build"], "API交接产品构建")?;
    let build = document::read(&path)?;
    let product = crate::recovery::clone::native::post_process::runtime::selected_product_root(
        backend, target, scope, &path, &build,
    )?;
    if crate::recovery::clone::native::post_process::runtime::product_root(backend, &runtime)?
        != product
    {
        return Err("API冻结产品根与目标及已登记构建不一致".into());
    }
    Ok(runtime)
}

pub(in crate::recovery::clone::native) fn cleanup_runtime(
    backend: &Path,
    run: &Path,
    post: &seed::history::Post,
) -> Result<Value> {
    recorded_runtime(backend, run, post, None, true)
}

fn recorded_runtime(
    backend: &Path,
    run: &Path,
    post: &seed::history::Post,
    api_url: Option<&Value>,
    cleanup: bool,
) -> Result<Value> {
    let saved = state::load(run, !cleanup)?;
    let mut latest = None;
    for row in state::attempts(&saved)?.iter().filter(|row| {
        row["stage"] == "post-copy" && row["mode"] == "prepare" && row["status"] == "passed"
    }) {
        let result = common::read_bound(backend, &row["result"], "API已发布prepare")?;
        let handoff = common::read_bound(backend, &result["handoff"], "API已发布handoff")?;
        if handoff["registration"] == post.descriptor {
            latest = Some((result, handoff));
        }
    }
    let (prepared, handoff) = latest.ok_or("活动post-copy登记没有对应的新prepare")?;
    if prepared["status"] != "post_copy_api_prepared"
        || prepared["worker_started"] != false
        || api_url.is_some_and(|api_url| handoff["api_url"] != *api_url)
        || prepared["handoff"] != common::binding(&post.runtime.join("handoff.json"))?
        || handoff["registration"] != post.descriptor
        || handoff["worker_started"] != false
        || handoff["runtime"] != common::binding(&post.runtime.join("runtime.json"))?
        || handoff["binaries"] != common::binding(&post.runtime.join("binaries.json"))?
    {
        return Err("API runtime交接、prepare发布或冻结文件不同于活动登记".into());
    }
    let frozen = common::read_bound(backend, &handoff["runtime"], "已发布运行合同")?;
    let product =
        crate::recovery::clone::native::post_process::runtime::product_root(backend, &frozen)?;
    let runtime =
        crate::recovery::fixture_control::verify_full_stack_runtime(&product, &post.runtime)?;
    if runtime != frozen {
        return Err("已发布运行合同在产品根核验期间变化".into());
    }
    if !cleanup {
        let build = common::read_bound(backend, &post.request["backend_build"], "API交接产品构建")?;
        for role in ["api", "worker"] {
            if runtime["artifacts"][role]
                != json!({"path":build["artifacts"][role]["executable"],
                    "sha256":build["artifacts"][role]["sha256"]})
            {
                return Err("API交接运行产物不同于活动请求绑定的API/Worker构建".into());
            }
        }
    }
    if state::load(run, !cleanup)? != saved {
        return Err("API交接prepare历史在读取期间变化".into());
    }
    Ok(runtime)
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/cache/post_registration.rs"]
mod tests;

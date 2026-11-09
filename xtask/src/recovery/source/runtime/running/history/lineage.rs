//! 失败谱系的只读授权消费；完整原像和后像必须实际读取，不能只比较旧收据。
use crate::recovery::source::evidence::{images, inventory, staging};
use crate::{
    Result,
    recovery::{
        clone::native::{json as document, resources::common, seed::generation},
        fixture_control,
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "lineage/failure.rs"]
pub(in crate::recovery::source::evidence) mod failure;
#[path = "lineage/retry.rs"]
mod retry;

pub(in crate::recovery::source::evidence) fn authority(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
) -> Result<Value> {
    let plan = model("replay-plan", json!({"attempts":attempts}))?;
    if plan["record"].is_null() {
        return Ok(Value::Null);
    }
    if document::boolean(&plan["fallback"], "谱系失败回退")? {
        if let Some(value) = super::completed::read(backend, run, attempts)? {
            return Ok(value);
        }
        return retry::authority(backend, run, attempts)?
            .ok_or_else(|| "C69 谱系恢复失败或未发布，不能授权重试".into());
    }
    let record = &plan["record"];
    if common::bound_file(backend, &record["result"], "C69 谱系恢复结果")?
        != run.join("results/0069.json")
    {
        return Err("C69 谱系恢复结果不属于固定相邻阶段".into());
    }
    let receipt = common::read_bound(backend, &record["result"], "C69 谱系恢复结果")?;
    generation::lineage_receipt(&receipt)?;
    let request = Path::new(document::text(
        &receipt["request"]["path"],
        "C69 正式请求路径",
    )?);
    let facts = failure::inspect(backend, run, attempts, Some(request))?;
    staging::validate_source(&record["sources"])?;
    model(
        "verify-receipt",
        json!({"receipt":receipt,"facts":{"request":facts["request"],
        "request_descriptor":facts["request_descriptor"],"proof":facts["proof"],"before":facts["before"],"failed":facts["failed"]},
        "prefix":plan["prefix"],"record":record}),
    )?;
    let observed = output(
        backend,
        run,
        &facts,
        &receipt["evidence_manifest"],
        &receipt["runtime"],
        &receipt["after"],
    )?;
    if facts["proof"] != failure::inspect(backend, run, attempts, Some(request))?["proof"] {
        return Err("C69 核验期间 C68 失败或完整前像变化".into());
    }
    common::bound_file(backend, &record["result"], "C69 谱系恢复结果")?;
    Ok(
        json!({"record":record,"failed":facts["failed"],"records":[facts["failed"],record],
        "request":receipt["request"],"receipt":receipt,"image":observed["image"],"failure":facts}),
    )
}

pub(in crate::recovery::source::evidence) fn model(action: &str, input: Value) -> Result<Value> {
    let value = generation::coordinator::dispatch(
        &json!({"action":"lineage","input":{"action":action,"input":input}}),
    )?;
    if let Some(reason) = value.get("rejected") {
        return Err(document::text(reason, "谱系模型拒绝原因")?
            .to_owned()
            .into());
    }
    Ok(value)
}

fn output(
    backend: &Path,
    run: &Path,
    facts: &Value,
    manifest: &Value,
    runtime_binding: &Value,
    after: &Value,
) -> Result<Value> {
    let output = run.join("seed-runtime/attempt-0069");
    let runtime = output.join("runtime");
    failure::inputs::exact_directory(backend, &output, &[], &["runtime", "after"])?;
    failure::inputs::exact_directory(backend, &runtime, &["binaries.json", "runtime.json"], &[])?;
    if common::bound_file(backend, runtime_binding, "C69 运行配置")? != runtime.join("runtime.json")
        || common::bound_file(backend, after, "C69 完整后像")? != output.join("after/image.json")
        || failure::summary(&inventory::manifest(&output)?)? != *manifest
    {
        return Err("C69 谱系恢复目录或证据绑定不同".into());
    }
    if document::read(&runtime.join("binaries.json"))?
        != failure::inputs::binaries(backend, &facts["request"])?
    {
        return Err("C69 运行二进制与绑定构建不同".into());
    }
    let execution = Path::new(document::text(
        &facts["request"]["execution_backend"],
        "C69 执行根",
    )?);
    if fixture_control::verify_full_stack_runtime(execution, &runtime)? != facts["contract"] {
        return Err("C69 运行配置与 C68 不同".into());
    }
    let mut selected = facts["selected"].clone();
    selected["runtime_dir"] = json!(runtime);
    let image = images::verify(
        backend,
        after,
        &selected,
        &facts["source"]["request"],
        &facts["request"]["source_registration"],
    )?;
    if image["image"] != facts["image"] {
        return Err("C69 完整后像与 C68 启动前像不同".into());
    }
    if failure::summary(&inventory::manifest(&output)?)? != *manifest {
        return Err("C69 核验期间完整证据目录变化".into());
    }
    common::bound_file(backend, runtime_binding, "C69 运行配置")?;
    common::bound_file(backend, after, "C69 完整后像")?;
    Ok(json!({"runtime":runtime_binding,"after":after,"image":image["image"]}))
}

//! START 沿原完整复制血缘与收据模型发布，不签发简化来源证明。
use super::*;
use crate::recovery::clone::native::seed::{history::post, source_registration::copied};

pub(super) fn lineage<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    before: &Value,
    image: &Value,
) -> Result<Value> {
    let frame = observation.frame;
    let source = &observation.prepared.facts["source"];
    let post = post(
        frame.backend,
        frame.run,
        Some(&source["registration"]["post_copy"]),
        None,
        false,
    )?;
    let plan = copied::require(frame.backend, frame.run, &source["manifest"])?;
    crate::recovery::source::evidence::lineage::derive(&json!({"backend":frame.backend,
        "source":{"review_successor":source["review_successor"],"result":source["result"],
        "registration":source["registration"],"manifest":source["manifest"],"request":source["request"],"directory":frame.run},
        "post_request":post.request,"post_descriptor":post.descriptor,"copy_plan":plan,
        "image_descriptor":before,"verified_image":image}))
}

pub(super) fn evidence<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    operations: &Value,
) -> Result<Value> {
    let runtime = observation.output()?.join("runtime");
    roles::directory(&runtime, operations)?;
    let mut records = serde_json::Map::new();
    for role in ["api", "worker"] {
        records.insert(
            role.into(),
            json!({"process":common::binding(&runtime.join(format!("{role}.json")))?,
            "tree":common::binding(&runtime.join(format!("{role}-tree.json")))?,"ready":true}),
        );
    }
    let value = json!({"format_version":1,"kind":"seed-source-runtime-running",
        "intent":common::binding(&observation.output()?.join("intent.json"))?,
        "runtime":common::binding(&runtime.join("runtime.json"))?,"roles":records,
        "physical_binding":observation.prepared.facts["source"]["generation"]["physical_binding"]});
    let intent = document::read(&observation.output()?.join("intent.json"))?;
    roles::observe(
        &runtime,
        &value,
        &intent,
        &observation.prepared.facts["selected"],
        &observation.contract,
        true,
    )?;
    document::write_new(&observation.output()?.join("running-evidence.json"), &value)?;
    Ok(value)
}

pub(super) fn receipt<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    before: &Value,
    running: &Value,
) -> Result<Value> {
    let output = observation.output()?;
    model(
        "start-receipt",
        json!({"request":observation.prepared.facts["request"],
        "descriptor":observation.prepared.descriptor,"coordinator_source":observation.frame.source(),
        "prefix":observation.frame.prefix,"generation_id":output.file_name().and_then(|s|s.to_str()).ok_or("START代次名称无效")?,
        "bindings":{"intent":common::binding(&output.join("intent.json"))?,
        "runtime_evidence":common::binding(&output.join("running-evidence.json"))?,"before":before,"running":running,
        "dataset_lineage":common::binding(&output.join("dataset-lineage.json"))?}}),
    )
}

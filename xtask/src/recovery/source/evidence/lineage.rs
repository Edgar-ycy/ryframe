//! 将绑定的原始数据、复制账本和当前完整像投影为唯一来源血缘。
use super::{binding::path, documents};
use crate::recovery::dataset_prepare::existing_native::source::validate_lineage;
use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[path = "lineage/chain.rs"]
mod chain;
#[path = "lineage/dataset.rs"]
mod dataset;
#[path = "lineage/fields.rs"]
mod fields;
#[path = "lineage/projection.rs"]
mod projection;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/evidence/lineage/tests.rs"]
mod tests;

struct Context {
    root: PathBuf,
    snapshots: Vec<(PathBuf, digest::FileDigest)>,
}
impl Context {
    fn read(&mut self, descriptor: &Value) -> Result<Value> {
        let path = common::bound_file(&self.root, descriptor, "来源血缘文件绑定")?;
        let before = digest::file(&path)?;
        let value =
            documents::read(&path).map_err(|_| "来源血缘文件不是严格JSON；不回显文件内容")?;
        self.snapshots.push((path, before));
        Ok(value)
    }
    fn file(&mut self, file: &Path) -> Result<(Value, Value)> {
        let descriptor = common::binding(file)?;
        let value = self.read(&descriptor)?;
        Ok((descriptor, value))
    }
    fn unchanged(&self) -> Result<()> {
        for (file, expected) in &self.snapshots {
            if digest::file(file)? != *expected {
                return Err("来源血缘核验期间祖先或当前像发生变化".into());
            }
        }
        Ok(())
    }
}

pub(super) fn derive(args: &Value) -> Result<Value> {
    derive_with(args, &|root, path, value| {
        crate::recovery::clone::native::issue(root, path, value).map(|_| ())
    })
}

fn derive_with(
    args: &Value,
    verifier: &impl Fn(&Path, &Path, &Value) -> Result<()>,
) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "source",
            "post_request",
            "post_descriptor",
            "copy_plan",
            "image_descriptor",
            "verified_image",
        ],
        "派生来源血缘参数",
    )?;
    documents::exact(
        &args["source"],
        &[
            "review_successor",
            "result",
            "registration",
            "manifest",
            "request",
            "directory",
        ],
        "来源血缘上下文",
    )?;
    let mut context = Context {
        root: path(&args["backend"])?,
        snapshots: Vec::new(),
    };
    let image = context.read(&args["image_descriptor"])?;
    if image != args["verified_image"] {
        return Err("当前完整像与已验证值不同".into());
    }
    let chain = chain::load(&mut context, args)?;
    let dataset = dataset::load(&mut context, &args["post_request"])?;
    let copy = chain::copy(&mut context, args, &dataset, verifier)?;
    let verification = chain::verification(&mut context, args, &chain, &dataset, &copy)?;
    let tenants = projection::tenants(&dataset.plan, &dataset.value, &image)?;
    let objects = projection::objects(&dataset.value, &args["copy_plan"], &image)?;
    let mut current_posts = 0_u64;
    for db in documents::object(&image["image"]["databases"], "当前完整数据库像")?.values()
    {
        for table in documents::array(&db["target"]["database"]["tables"], "当前表库存")? {
            if table["table"] == "sys_post" {
                current_posts = current_posts
                    .checked_add(table["rows"].as_u64().ok_or("岗位行数无效")?)
                    .ok_or("岗位行数溢出")?;
            }
        }
    }
    let expected = documents::array(&args["copy_plan"]["databases"], "复制数据库目标")?
        .iter()
        .try_fold(0_u64, |total, db| {
            total
                .checked_add(
                    db["tables"]
                        .get("sys_post")
                        .map_or(Some(0), Value::as_u64)
                        .ok_or("复制岗位行数无效")?,
                )
                .ok_or("复制岗位行数溢出")
        })?;
    if current_posts != expected
        || current_posts < dataset.value["records"].as_u64().ok_or("原始数据量无效")?
    {
        return Err("当前完整像的sys_post总数与已验证复制计划不同".into());
    }
    let request = &args["post_request"];
    let mut value = json!({"format_version":1,"kind":"restore-source-derived-dataset-lineage","status":"derived_dataset_verified",
        "source_registration":args["source"]["review_successor"]["source_result"],"seed_registration":chain["registration"],
        "post_copy":args["post_descriptor"],"post_verify":verification["result"],"post_verify_evidence":verification["evidence"],
        "post_verify_target":verification["target"],"copy_plan":copy["plan"],"current_image":args["image_descriptor"],
        "scopes":{"origin_tenant_scope_id":dataset.value["source_scope_id"],"current_source_scope_id":args["source"]["request"]["source"]["scope_id"],
            "current_object_scope_id":args["copy_plan"]["target_scope"]},
        "scale":{"records":dataset.value["records"],"current_post_rows":current_posts,"tenants":tenants.len(),
            "post_samples":dataset.post_samples,"business_objects":objects["business_objects"],"verified_objects":objects["verified_objects"],"object_bytes":objects["business_bytes"]},
        "tenants":tenants,"objects":objects,"verification":verification["target_value"],"restore_qualified":false});
    value["verification"]["request_interval_ms"] = dataset.value["request_interval_ms"].clone();
    for field in [
        "reference_plan",
        "dataset",
        "copy_stage_receipt",
        "copy_result",
        "ledger_head",
    ] {
        value[field] = request[field].clone();
    }
    validate_lineage(&value)?;
    context.unchanged()?;
    Ok(value)
}

pub(super) fn verify(args: &Value) -> Result<Value> {
    verify_with(args, &|root, path, value| {
        crate::recovery::clone::native::issue(root, path, value).map(|_| ())
    })
}
fn verify_with(
    args: &Value,
    verifier: &impl Fn(&Path, &Path, &Value) -> Result<()>,
) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "source",
            "post_request",
            "post_descriptor",
            "copy_plan",
            "image_descriptor",
            "verified_image",
            "lineage_descriptor",
        ],
        "来源血缘复验参数",
    )?;
    let root = path(&args["backend"])?;
    let mut context = Context {
        root,
        snapshots: Vec::new(),
    };
    let value = context.read(&args["lineage_descriptor"])?;
    validate_lineage(&value)?;
    let mut request = args.clone();
    request
        .as_object_mut()
        .ok_or("血缘参数无效")?
        .remove("lineage_descriptor");
    if value != derive_with(&request, verifier)? {
        return Err("派生数据血缘与祖先、复制账本或当前完整像不同".into());
    }
    context.unchanged()?;
    Ok(value)
}

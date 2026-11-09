use super::{Context, dataset::Dataset, documents, fields};
use crate::{
    Result,
    recovery::clone::native::{
        digest, input::BUCKETS, resources::common, s3::protocol::validate_key,
    },
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn endpoint(value: &Value) -> Result<()> {
    let url =
        url::Url::parse(value.as_str().ok_or("血缘端点必须是文本")?).map_err(|_| "血缘端点无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || url.port_or_known_default().is_none_or(|port| port == 0)
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("血缘端点必须是明确本机根地址".into());
    }
    // URL解析会折叠默认端口，仍要求原始文本显式写出端口。
    let authority = value
        .as_str()
        .ok_or("血缘端点缺失")?
        .split("://")
        .nth(1)
        .ok_or("血缘端点缺少协议")?
        .trim_end_matches('/');
    if authority.contains('@')
        || authority.rsplit_once(':').is_none_or(|(_, port)| {
            port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err("血缘端点必须显式声明端口".into());
    }
    Ok(())
}

pub(super) fn load(context: &mut Context, args: &Value) -> Result<Value> {
    let source = &args["source"];
    let outer = context.read(&source["review_successor"]["source_result"])?;
    documents::exact(&outer, fields::SOURCE_RESULT_FIELDS, "来源外层结果")?;
    let registration = context.read(&outer["registration"])?;
    documents::exact(
        &registration,
        fields::SOURCE_REGISTRATION_FIELDS,
        "来源内层登记",
    )?;
    if outer != source["result"]
        || registration != source["registration"]
        || outer["status"] != "seed_source_registered"
        || outer["remote_writes"] != 0
        || outer["outbox_drained"] != true
        || outer["restore_qualified"] != false
        || registration["remote_writes"] != 0
        || registration["outbox_drained"] != true
        || registration["restore_qualified"] != false
        || registration["source_request"] != outer["source_request"]
        || registration["post_copy"] != args["post_descriptor"]
        || args["verified_image"]["source_registration"]
            != source["review_successor"]["source_result"]
    {
        return Err("来源外层结果、内层登记或当前完整像绑定不同".into());
    }
    let directory = super::super::binding::path(&source["directory"])?;
    if registration["format_version"] != 1
        || registration["kind"] != "devex-clone-seed-source-registration"
        || registration["run_directory"] != source["directory"]
        || context.read(&registration["source_request"])? != source["request"]
        || context.read(&registration["run_manifest"])? != source["manifest"]
        || common::bound_file(&context.root, &registration["run_manifest"], "来源运行清单")?
            != directory.join("manifest.json")
    {
        return Err("来源请求、运行目录或原清单与已登记文件不同".into());
    }
    let post = context.read(&args["post_descriptor"])?;
    if post.get("kind").and_then(Value::as_str) == Some("devex-clone-post-copy-amendment") {
        let directory = super::super::binding::path(&source["directory"])?;
        let (_, mut request) = context.file(&directory.join("post-copy.json"))?;
        let mut previous = common::binding(&directory.join("post-copy.json"))?;
        let sequence = post["sequence"]
            .as_u64()
            .filter(|value| *value > 0 && *value <= 9999)
            .ok_or("post-copy修正代次无效")?;
        for index in 1..=sequence {
            let (descriptor, value) = context.file(
                &directory
                    .join("post-copy/amendments")
                    .join(format!("{index:04}.json")),
            )?;
            documents::exact(
                &value,
                &[
                    "format_version",
                    "kind",
                    "sequence",
                    "run_manifest",
                    "predecessor",
                    "request_sha256",
                    "api_environment",
                    "invalidated_runtime",
                ],
                "post-copy修正",
            )?;
            if value["format_version"] != 1
                || value["kind"] != "devex-clone-post-copy-amendment"
                || value["sequence"] != index
                || value["predecessor"] != previous
                || value["run_manifest"] != common::binding(&directory.join("manifest.json"))?
            {
                return Err("post-copy修正顺序或来源无效".into());
            }
            context.read(&value["api_environment"])?;
            request["api_environment"] = value["api_environment"].clone();
            if value["request_sha256"] != digest::plan_hash(&request)? {
                return Err("post-copy修正不能还原完整请求".into());
            }
            previous = descriptor;
        }
        if previous != args["post_descriptor"] || request != args["post_request"] {
            return Err("post-copy修正与已验证请求不同".into());
        }
    } else if post != args["post_request"] {
        return Err("post-copy原始请求与绑定内容不同".into());
    }
    Ok(json!({"registration":outer["registration"],"value":registration}))
}

pub(super) fn copy(
    context: &mut Context,
    args: &Value,
    dataset: &Dataset,
    verifier: &impl Fn(&std::path::Path, &std::path::Path, &Value) -> Result<()>,
) -> Result<Value> {
    let request = &args["post_request"];
    let result = context.read(&request["copy_result"])?;
    context.read(&request["copy_stage_receipt"])?;
    context.read(&request["ledger_head"])?;
    let path = common::bound_file(&context.root, &request["copy_result"], "复制结果")?
        .with_file_name("plan.json");
    let (descriptor, wrapper) = context.file(&path)?;
    documents::exact(
        &wrapper,
        &["format_version", "input_path", "input_file", "plan"],
        "复制计划完整包装",
    )?;
    if wrapper["plan"] != args["copy_plan"] {
        return Err("已验证复制计划与绑定文件不同".into());
    }
    // 复用原生完整计划签发，不能只接受调用方传入的计划JSON。
    verifier(&context.root, &path, &wrapper)?;
    let plan = &args["copy_plan"];
    if result["status"] != "data_steps_verified"
        || result["plan_sha256"] != plan["plan_sha256"]
        || plan["copy_stage"] != "source_to_seed"
        || plan["copy_stage"] != args["source"]["manifest"]["copy_stage"]
        || plan["source_scope"] != dataset.plan["source"]["scope_id"]
        || plan["target_scope"] != args["source"]["request"]["source"]["scope_id"]
    {
        return Err("复制计划、账本或来源目标scope不同".into());
    }
    let value = json!({"fresh_target_sha256":result["fresh_target_sha256"],"generation_sha256":result["generation_sha256"],
        "ledger_head_sha256":request["ledger_head"]["sha256"],"plan_sha256":plan["plan_sha256"],
        "source_export_sha256":result["source_export_sha256"],"stage_receipt_sha256":request["copy_stage_receipt"]["sha256"]});
    for field in value.as_object().ok_or("复制绑定无效")?.values() {
        if field.as_str().is_none_or(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            return Err("复制结果缺少明确摘要".into());
        }
    }
    Ok(json!({"plan":descriptor,"value":value}))
}

pub(super) fn verification(
    context: &mut Context,
    args: &Value,
    chain: &Value,
    dataset: &Dataset,
    copy: &Value,
) -> Result<Value> {
    let descriptor = &chain["value"]["post_verify"];
    let result = context.read(descriptor)?;
    documents::exact(&result, fields::POST_RESULT_FIELDS, "原业务复验外层结果")?;
    let directory = super::super::binding::path(&args["source"]["directory"])?;
    let (_, state) = context.file(&directory.join("state.json"))?;
    let mut matched = Vec::new();
    for attempt in documents::array(&state["attempts"], "来源阶段历史")? {
        if attempt["stage"] == "post-copy"
            && attempt["mode"] == "verify"
            && attempt["status"] == "passed"
            && attempt["result"] == *descriptor
        {
            matched.push(attempt);
        }
    }
    if matched.len() != 1
        || common::bound_file(&context.root, descriptor, "业务复验结果")?
            != directory.join("results").join(format!(
                "{:04}.json",
                matched[0]["number"].as_u64().ok_or("业务复验阶段号无效")?
            ))
    {
        return Err("业务复验不是同一copy-run唯一已发布阶段".into());
    }
    let evidence = context.read(&result["evidence"])?;
    documents::exact(&evidence, fields::BUSINESS_FIELDS, "原业务复验")?;
    let target_path = common::bound_file(&context.root, &result["evidence"], "业务复验")?
        .with_file_name("business-target.json");
    let (target_descriptor, target) = context.file(&target_path)?;
    documents::exact(&target, fields::BUSINESS_TARGET_FIELDS, "原业务复验目标")?;
    documents::exact(
        &target["target"],
        &["scope_id", "api_url", "frontend_url"],
        "业务复验端点",
    )?;
    endpoint(&target["target"]["api_url"])?;
    endpoint(&target["target"]["frontend_url"])?;
    let source = &args["source"]["request"]["source"];
    verify_object_coverage(
        &result["objects"],
        &dataset.value,
        dataset.files,
        &args["copy_plan"],
    )?;
    if result["status"] != "post_copy_existing_data_verified"
        || result["registration"] != args["post_descriptor"]
        || result["worker_must_remain_stopped"] != true
        || result["restore_qualified"] != false
        || evidence["format_version"] != 1
        || evidence["status"] != "copy_existing_data_verified"
        || evidence["side"] != "copy_target"
        || evidence["clone_verified"] != false
        || evidence["restore_success"] != false
        || evidence["actions"]
            != json!({"business":"read_only","objects":"read_only","session":"login_logout"})
        || evidence["scope_id"] != source["scope_id"]
        || evidence["source_scope_id"] != dataset.value["source_scope_id"]
        || evidence["plan_sha256"] != dataset.value["plan_sha256"]
        || evidence["input_files"]
            != json!({"plan_sha256":args["post_request"]["reference_plan"]["sha256"],"dataset_sha256":args["post_request"]["dataset"]["sha256"],"target_binding_sha256":target_descriptor["sha256"]})
        || evidence["copy"] != copy["value"]
        || evidence["target_binding_sha256"] != digest::plan_hash(&target)?
        || evidence["tenants"] != 11
        || evidence["posts"] != dataset.post_samples
        || evidence["files"] != dataset.files
        || target["format_version"] != 1
        || target["kind"] != "devex-copy-business-target"
        || target["copy"] != copy["value"]
        || target["source_plan_sha256"] != dataset.value["plan_sha256"]
        || target["source_scope_id"] != dataset.value["source_scope_id"]
        || target["target"]["scope_id"] != source["scope_id"]
        || target["target"]["api_url"] != source["api_url"]
    {
        return Err("原业务复验没有精确绑定原样本、复制账本或当前scope".into());
    }
    Ok(
        json!({"result":descriptor,"evidence":result["evidence"],"target":target_descriptor,"target_value":target["target"]}),
    )
}

fn verify_object_coverage(
    receipt: &Value,
    dataset: &Value,
    files: u64,
    copy_plan: &Value,
) -> Result<()> {
    let scope = copy_plan["target_scope"]
        .as_str()
        .ok_or("原复制目标对象 scope 缺失")?;
    let mut remaining = BTreeMap::new();
    for item in documents::array(&copy_plan["objects"], "原复制计划对象")? {
        let bucket = item["bucket"].as_str().ok_or("原复制对象桶无效")?;
        let key = item["target_key"]
            .as_str()
            .ok_or("原复制目标对象 key 无效")?;
        validate_key(scope, key, false)?;
        if !BUCKETS.contains(&bucket)
            || remaining
                .insert((bucket.to_owned(), key.to_owned()), &item["artifact"])
                .is_some()
        {
            return Err("原复制目标对象桶或 key 重复或无效".into());
        }
    }
    let mut matched = 0_u64;
    for tenant in documents::array(&dataset["tenants"], "原业务租户")? {
        for file in documents::array(&tenant["files"], "原业务对象样本")? {
            let relative = file["file_path"].as_str().ok_or("原业务对象路径缺失")?;
            let key = format!("{scope}/{relative}");
            validate_key(scope, &key, false)?;
            let artifact = remaining
                .remove(&("uploads".to_owned(), key))
                .ok_or("原业务对象重复或未匹配原复制计划 uploads")?;
            if artifact["bytes"] != file["bytes"] || artifact["sha256"] != file["sha256"] {
                return Err("原业务对象载荷与原复制计划不同".into());
            }
            matched += 1;
        }
    }
    // 这里只核对历史 post-copy 观察覆盖，额外对象由完整像门禁另行核对。
    if matched != files
        || *receipt
            != json!({
                "business_objects":matched, "additional_objects":remaining.len(),
                "additional_objects_downloaded":true, "all_scoped_keys_verified":true,
            })
    {
        return Err("原业务复验没有覆盖同一复制计划的完整对象范围和精确补集".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/evidence/lineage/object_coverage.rs"]
mod object_coverage_tests;

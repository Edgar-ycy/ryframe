//! 配额动作的不可变意图、完整后像及已知/未知 PUT 结果证据。
use std::path::Path;

use serde_json::{Value, json};

use super::{inputs::Reads, model};
use crate::{
    Result,
    recovery::clone::native::{json as document, seed::history as seed_history},
};

const EDITABLE: &[&str] = &[
    "name",
    "domain",
    "expire_at",
    "max_users",
    "max_roles",
    "max_storage_mb",
    "max_requests_per_min",
];

fn intent(reads: &mut Reads<'_>, run: &Path, item: &Value, attempt: &Path) -> Result<()> {
    let value = reads.read(&attempt.join("intent.json"))?;
    let tenant = document::object(&item["after"]["tenant"], "配额完整后像")?;
    let body = EDITABLE
        .iter()
        .map(|field| {
            Ok((
                (*field).to_owned(),
                tenant.get(*field).ok_or("配额后像缺少可编辑字段")?.clone(),
            ))
        })
        .collect::<Result<serde_json::Map<_, _>>>()?;
    let expected = json!({"plan":reads.binding(&run.join("seed-runtime/quota-plan.json"))?,
        "target":item["target"],"before":reads.binding(&attempt.join("before.json"))?,
        "body":body,"operation":"put_platform_tenants_by_tenant_id","automatic_retry":false});
    if value != expected
        || model::classification(item, &reads.bound(&value["before"])?)? != "before"
    {
        return Err("配额写入意图或完整前像不同于明确计划".into());
    }
    Ok(())
}

pub(super) fn confirmation(
    reads: &mut Reads<'_>,
    run: &Path,
    plan: &Value,
    item: &Value,
    attempt: &Path,
) -> Result<Option<seed_history::Post>> {
    let value = reads.read(&attempt.join("confirmed.json"))?;
    document::exact(
        &value,
        &[
            "plan",
            "target",
            "observed",
            "api_response_proven",
            "put_attempted",
            "evidence",
        ],
        "配额确认",
    )?;
    let proven = document::boolean(&value["api_response_proven"], "配额 API 成功标记")?;
    let attempted = document::boolean(&value["put_attempted"], "配额 PUT 尝试标记")?;
    if value["plan"] != reads.binding(&run.join("seed-runtime/quota-plan.json"))?
        || value["target"] != item["target"]
        || model::classification(item, &value["observed"])? != "after"
    {
        return Err("配额确认缺少完整后像".into());
    }
    let mut required = vec!["observed"];
    if attempted {
        required.extend([
            "intent",
            "before",
            if proven { "api-response" } else { "reconcile" },
        ]);
    } else if proven || item["before"] != item["after"] {
        return Err("只读确认不能伪造 PUT 成功".into());
    }
    let files = document::exact(&value["evidence"], &required, "配额确认证据集合")?;
    for (name, descriptor) in files {
        let path = crate::recovery::clone::native::resources::common::bound_file(
            reads.backend,
            descriptor,
            "配额确认证据",
        )?;
        if path.parent() != Some(attempt) || !evidence_name(name, &path) {
            return Err("配额证据越界".into());
        }
        reads.bound(descriptor)?;
    }
    let post = if attempted {
        intent(reads, run, item, attempt)?;
        if proven {
            let response = reads.read(&attempt.join("api-response.json"))?;
            if *model::data(&response["response"])? != item["after"]["tenant"] {
                return Err("配额 API 响应与完整后像不符".into());
            }
            let registration = reads.bound(&plan["registration"])?;
            reads.bound(&registration["post_copy"])?;
            let post = seed_history::post(
                reads.backend,
                run,
                Some(&registration["post_copy"]),
                None,
                false,
            )?;
            model::authorization(&post.request, &response["authorization"])?;
            Some(post)
        } else {
            let proof = reads.bound(&value["evidence"]["reconcile"])?;
            if proof
                != json!({"intent":reads.binding(&attempt.join("intent.json"))?,
                "observed":value["evidence"]["observed"],"state":"after"})
            {
                return Err("未知配额写入缺少当前意图的后像核对".into());
            }
            None
        }
    } else {
        None
    };
    if reads.bound(&value["evidence"]["observed"])? != value["observed"] {
        return Err("配额确认的原始观察不同于完整后像".into());
    }
    Ok(post)
}

fn evidence_name(name: &str, path: &Path) -> bool {
    let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if ["intent", "before", "api-response"].contains(&name) {
        return file == format!("{name}.json");
    }
    file.strip_prefix(&format!("{name}-"))
        .and_then(|suffix| suffix.strip_suffix(".json"))
        .is_some_and(|number| number.len() == 4 && number.bytes().all(|byte| byte.is_ascii_digit()))
}

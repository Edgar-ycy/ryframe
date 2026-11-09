use super::documents;
use crate::{
    Result,
    recovery::clone::native::{digest, input::BUCKETS},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn tenants(plan: &Value, dataset: &Value, image: &Value) -> Result<Vec<Value>> {
    let targets = documents::array(&plan["dataset"]["tenant_targets"], "原计划租户目标")?;
    if targets.len() != 10
        || !targets.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.starts_with("dedicated-"))
        })
        || !targets
            .iter()
            .any(|value| matches!(value.as_str(), Some("shared" | "shared-control")))
    {
        return Err("原计划没有同时覆盖共享与独立的十个普通租户".into());
    }
    let origin = dataset["source_scope_id"]
        .as_str()
        .ok_or("原租户scope缺失")?;
    let by_id = documents::array(&dataset["tenants"], "原数据租户")?
        .iter()
        .map(|value| {
            Ok((
                value["tenant_id"].as_str().ok_or("租户ID缺失")?.to_owned(),
                value,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut expected = BTreeMap::from([("system".to_owned(), "shared-control".to_owned())]);
    for (index, target) in targets.iter().enumerate() {
        expected.insert(
            format!("{origin}-{:02}", index + 1),
            target.as_str().ok_or("租户目标无效")?.into(),
        );
    }
    let mut observed = BTreeMap::new();
    for (key, value) in documents::object(&image["image"]["databases"], "当前数据库库存")? {
        let db = &value["target"]["database"];
        if db["key"] != key.as_str() {
            return Err("当前完整像数据库逻辑键不一致".into());
        }
        for placement in documents::array(&db["placements"], "租户placement")? {
            let tenant = placement["tenant_id"]
                .as_str()
                .ok_or("租户placement缺少身份")?;
            if observed.insert(tenant.to_owned(), key.clone()).is_some() {
                return Err("当前完整像重复登记租户placement".into());
            }
        }
    }
    if observed != expected {
        return Err("当前完整像未保留十一租户的共享与独立placement".into());
    }
    let order = std::iter::once("system".into())
        .chain((1..=10).map(|index| format!("{origin}-{index:02}")));
    order
        .map(|id| {
            let tenant = by_id.get(&id).ok_or("原数据集租户缺失")?;
            Ok(
                json!({"tenant_id":id,"database":expected[&id],"username":tenant["username"],
            "records":tenant["records"],"posts":tenant["posts"],"files":tenant["files"]}),
            )
        })
        .collect()
}

pub(super) fn objects(dataset: &Value, copy: &Value, image: &Value) -> Result<Value> {
    let origin = dataset["source_scope_id"]
        .as_str()
        .ok_or("原对象scope缺失")?;
    let current = copy["target_scope"].as_str().ok_or("当前对象scope缺失")?;
    let mut declared = BTreeMap::new();
    for tenant in documents::array(&dataset["tenants"], "原数据租户")? {
        let id = tenant["tenant_id"].as_str().ok_or("原租户ID缺失")?;
        for file in documents::array(&tenant["files"], "原业务对象")? {
            let key = file["file_path"].as_str().ok_or("原业务对象路径缺失")?;
            if declared
                .insert((id.to_owned(), key.to_owned()), file)
                .is_some()
            {
                return Err("原业务对象重复".into());
            }
        }
    }
    let objects = documents::exact(&image["image"]["objects"], BUCKETS, "当前完整对象库存")?;
    let mut business = Vec::new();
    let mut additional = Vec::new();
    let mut planned = BTreeSet::new();
    for item in documents::array(&copy["objects"], "复制计划对象")? {
        documents::exact(
            item,
            &["bucket", "source_key", "target_key", "artifact", "metadata"],
            "复制计划对象",
        )?;
        let bucket = item["bucket"].as_str().ok_or("复制桶缺失")?;
        let source = item["source_key"].as_str().ok_or("复制来源键缺失")?;
        let target = item["target_key"].as_str().ok_or("复制目标键缺失")?;
        let relative = source
            .strip_prefix(&format!("{origin}/"))
            .ok_or("复制来源对象scope无效")?;
        if !BUCKETS.contains(&bucket)
            || !planned.insert((bucket.to_owned(), target.to_owned()))
            || target != format!("{current}/{relative}")
        {
            return Err("复制计划对象桶、scope或目标键重复无效".into());
        }
        let observed = &image["image"]["objects"][bucket][target];
        if !observed.is_object()
            || observed["bytes"] != item["artifact"]["bytes"]
            || observed["sha256"] != item["artifact"]["sha256"]
            || observed["metadata"] != item["metadata"]
        {
            return Err("当前完整像没有精确保留复制计划对象及scope重写".into());
        }
        let projection = json!({"bucket":bucket,"source_key":source,"target_key":target,"bytes":item["artifact"]["bytes"],"sha256":item["artifact"]["sha256"],"metadata":item["metadata"]});
        let tenant = relative.split('/').next().ok_or("业务对象租户缺失")?;
        if let Some(sample) = declared.remove(&(tenant.into(), relative.into())) {
            if sample["bytes"] != item["artifact"]["bytes"]
                || sample["sha256"] != item["artifact"]["sha256"]
            {
                return Err("复制计划业务对象与原数据集摘要不同".into());
            }
            business.push(projection);
        } else {
            additional.push(projection);
        }
    }
    let mut observed = BTreeSet::new();
    for (bucket, entries) in objects {
        for key in documents::object(entries, "对象桶库存")?.keys() {
            observed.insert((bucket.clone(), key.clone()));
        }
    }
    if !declared.is_empty() || business.len() != 256 || additional.len() > 1 {
        return Err("需要完整的256个业务对象，额外探针最多一个".into());
    }
    if observed != planned {
        return Err("当前完整像包含复制计划之外的未知对象".into());
    }
    business.sort_by(|a, b| {
        a["bucket"]
            .as_str()
            .cmp(&b["bucket"].as_str())
            .then_with(|| a["target_key"].as_str().cmp(&b["target_key"].as_str()))
    });
    let bytes = business.iter().try_fold(0_u64, |sum, item| {
        sum.checked_add(item["bytes"].as_u64().ok_or("业务对象字节无效")?)
            .ok_or("业务对象字节溢出")
    })?;
    Ok(
        json!({"business_objects":business.len(),"business_bytes":bytes,"mapping_sha256":digest::plan_hash(&json!(business))?,"probe":additional.first(),"verified_objects":planned.len()}),
    )
}

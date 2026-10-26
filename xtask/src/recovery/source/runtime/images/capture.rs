//! 运行中完整像按既有次序采集；每次实际端口前后仍由父控制区间复核。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{document, objects, schema};
use crate::{
    Result,
    recovery::{
        clone::native::{
            bridge::scope::Scope,
            digest,
            export::{database, input::Context, inventory, object_owner, transport::Transport},
            input::BUCKETS,
            resources::common,
        },
        source::evidence::resources::Resources,
    },
};

pub(in crate::recovery::source::evidence) fn run<T: Transport + Sync>(
    resources: &Resources<'_, T>,
    context: &Context,
    source: &Value,
    scope: &Scope,
) -> Result<Value> {
    let output = context.output();
    if output != resources.output
        || context.execution_backend != resources.execution
        || context.backend != resources.backend
        || context.environment != *resources.environment
    {
        return Err("来源完整像的执行根、输出和环境必须属于同一控制区间".into());
    }
    (resources.checkpoint)()?;
    fs::create_dir(&output)?;
    let run = Path::new(document::text(&source["directory"], "原复制账本")?);
    let storage = resources.storage_identity(run)?;
    let redis = resources.redis_state()?;
    let models = schema::models(resources.execution)?;
    let observed_at = chrono::Utc::now().to_rfc3339();
    database::verify_migrations(context, resources.transport, "migrations")?;
    let columns = database::schema_snapshot(context, &models, resources.transport, "before")?;
    let captured = resources.capture_databases(
        &output.join("databases"),
        context.source(),
        &context.request["tools"],
        &context.request["maintenance_build"],
        scope,
    )?;
    let owners =
        object_owner::owner_proofs(context, resources.transport, &output, "owners-before")?;
    let before = inventory::capture(
        context,
        resources.transport,
        &models,
        "before",
        &observed_at,
        true,
    )?;
    let evidence = objects::capture::run(context, resources.transport, &before)?;
    let after = inventory::capture(
        context,
        resources.transport,
        &models,
        "after",
        &observed_at,
        true,
    )?;
    let repeated =
        object_owner::owner_proofs(context, resources.transport, &output, "owners-after")?;
    if inventory::logical(&before)? != inventory::logical(&after)?
        || owner_image(&owners)? != owner_image(&repeated)?
        || resources.storage_identity(run)? != storage
        || resources.redis_state()? != redis
        || database::schema_snapshot(context, &models, resources.transport, "after")? != columns
    {
        return Err("来源采集期间完整库存、schema、对象 ownership、Redis 或存储代次变化".into());
    }
    let image = json!({"databases":captured["databases"],"schema":columns,
        "objects":object_images(resources.backend, &evidence)?,"owners":owner_image(&owners)?,
        "redis":redis,"storage":storage});
    let value = receipt(context, source, &image, &evidence, &owners, &repeated)?;
    scope.assert_unchanged()?;
    (resources.checkpoint)()?;
    document::write_new(&output.join("image.json"), &value)?;
    let binding = common::binding(&output.join("image.json"))?;
    super::verify(
        resources.backend,
        &binding,
        context.source(),
        &source["request"],
        &source["review_successor"]["source_result"],
    )?;
    (resources.checkpoint)()?;
    Ok(binding)
}

fn owner_image(rows: &[Value]) -> Result<Vec<Value>> {
    rows.iter().map(|row| {
        for field in ["bucket", "key", "bytes", "sha256"] {
            row.get(field).ok_or("来源 ownership 证据缺少固定字段")?;
        }
        Ok(json!({"bucket":row["bucket"],"key":row["key"],"bytes":row["bytes"],"sha256":row["sha256"]}))
    }).collect()
}

fn object_images(backend: &Path, descriptors: &[Value]) -> Result<Value> {
    let mut result = BUCKETS
        .iter()
        .map(|bucket| ((*bucket).to_owned(), json!({})))
        .collect::<BTreeMap<_, _>>();
    for descriptor in descriptors {
        let value = common::read_bound(backend, descriptor, "来源对象完整证据")?;
        let bucket = document::text(&value["bucket"], "来源对象桶")?;
        let key = document::text(&value["key"], "来源对象键")?;
        let entries = result
            .get_mut(bucket)
            .and_then(Value::as_object_mut)
            .ok_or("来源对象证据含未登记桶")?;
        if entries.insert(key.to_owned(), json!({"bytes":value["artifact"]["bytes"],
            "sha256":value["artifact"]["sha256"],"metadata":value["metadata"],"identity":value["identity"]})).is_some()
        {
            return Err("来源完整像含重复对象".into());
        }
    }
    Ok(serde_json::to_value(result)?)
}

fn receipt(
    context: &Context,
    source: &Value,
    image: &Value,
    objects: &[Value],
    owners: &[Value],
    repeated: &[Value],
) -> Result<Value> {
    let output = context.output();
    let mut raw = Vec::new();
    let mut schemas = Vec::new();
    for phase in ["before", "after"] {
        for key in ["shared-control", "shared", "dedicated-a", "dedicated-b"] {
            raw.push(common::binding(
                &output.join(format!("databases/{phase}-target-{key}.json")),
            )?);
        }
        for database in context.databases()? {
            let key = document::text(&database["key"], "来源 schema 目标")?;
            schemas.push(common::binding(
                &output.join(format!("schema-{phase}-{key}.json")),
            )?);
        }
    }
    let phase_bindings = |stem: &str| -> Result<Vec<Value>> {
        ["before", "after"]
            .iter()
            .map(|phase| common::binding(&output.join(format!("inventory-{phase}{stem}.json"))))
            .collect()
    };
    let owners = owners
        .iter()
        .chain(repeated)
        .map(|row| {
            common::binding(&output.join(document::text(&row["file"], "ownership 原始文件")?))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"format_version":1,"kind":"seed-source-generation-image",
        "source_registration":source["review_successor"]["source_result"],
        "inventory":common::binding(&output.join("databases/inventory.json"))?,"raw_inventories":raw,
        "object_evidence":objects,"owner_evidence":owners,"logical_inventories":phase_bindings("")?,
        "inventory_origins":phase_bindings("-targets")?,"schema_evidence":schemas,"image":image,
        "image_sha256":digest::plan_hash(image)?,"scale":super::scale(image)?}),
    )
}

/// 由实际读取的同代事实构造采集上下文；配置和环境来自持锁资源窗口。
pub(in crate::recovery::source::evidence) fn context<T: Transport>(
    resources: &Resources<'_, T>,
    facts: &Value,
) -> Result<Context> {
    let source = &facts["source"];
    let mut request = source["request"].clone();
    request["source"] = facts["selected"].clone();
    for key in ["id", "backend_build", "maintenance_build"] {
        request[key] = facts["request"][key].clone();
    }
    let context = Context {
        backend: resources.backend.to_owned(),
        run_dir: PathBuf::from(document::text(&facts["directory"], "来源账本")?),
        output_dir: resources.output.to_owned(),
        mode: "source-runtime".into(),
        attempt: facts["number"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("来源 START 序号无效")?,
        manifest: source["manifest"].clone(),
        object_identity:
            crate::recovery::clone::native::s3::app_configuration::public_configuration(
                resources.backend,
                &request["source"]["s3"]["configuration"],
            )?,
        request,
        request_path: PathBuf::from(document::text(
            &source["registration"]["source_request"]["path"],
            "原来源请求",
        )?),
        environment: resources.environment.clone(),
        execution_backend: resources.execution.to_owned(),
    };
    Ok(context)
}

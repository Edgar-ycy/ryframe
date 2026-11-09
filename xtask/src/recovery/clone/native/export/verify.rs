use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    digest,
    json::{self as json_io, exact},
    lifecycle::manifest,
    target_verify::guard::Inputs,
};
use super::{database, input::Context, inventory, proof};
use crate::Result;

pub(crate) mod frozen;

pub(crate) fn binding(context: &Context, descriptor: &Value) -> Result<Value> {
    let binding = manifest::validate_binding_shape(descriptor, "源导出绑定")?;
    let path = PathBuf::from(
        binding
            .get("path")
            .and_then(Value::as_str)
            .ok_or("源导出绑定缺少路径")?,
    );
    let actual = digest::file(&path)?;
    if actual.bytes != binding["bytes"].as_u64().unwrap_or_default()
        || actual.sha256 != binding["sha256"].as_str().unwrap_or_default()
        || path.file_name().and_then(|value| value.to_str()) != Some("export.json")
    {
        return Err("源导出绑定与 export.json 摘要不一致".into());
    }
    let failure = path.parent().unwrap_or(Path::new(".")).join("failure.json");
    if failure.exists() {
        return Err("源证据包含失败标记，不能使用残留成功文件".into());
    }
    let value = json_io::read(&path)?;
    validate_export(context, &value, &path)?;
    let root = path.parent().ok_or("源导出缺少证据目录")?;
    Ok(json!({
        "export": value,
        "binding": descriptor,
        "logical_inventory_sha256": value["logical_inventory_sha256"],
        "generation": json_io::read(&root.join("generation-before.json"))?,
        "proof_files": super::evidence::files(context, &value, root)?,
    }))
}

fn validate_export(context: &Context, value: &Value, path: &Path) -> Result<()> {
    let artifact_root = PathBuf::from(
        value
            .get("artifact_root")
            .and_then(Value::as_str)
            .ok_or("源导出缺少证据根目录")?,
    );
    let root = validate_shape(
        &context.backend,
        &context.request,
        &context.request_path,
        value,
        path,
        &artifact_root,
    )?;
    validate_evidence(context, root, &artifact_root)?;
    let models = database::models(context)?;
    validate_export_databases(context, root, &artifact_root, &models)?;
    validate_export_objects(root, &artifact_root)?;
    validate_generation(context, root, &artifact_root)
}

fn validate_shape<'a>(
    backend: &Path,
    request: &Value,
    request_path: &Path,
    value: &'a Value,
    path: &Path,
    artifact_root: &Path,
) -> Result<&'a serde_json::Map<String, Value>> {
    let fields = [
        "format_version",
        "kind",
        "status",
        "id",
        "scope_id",
        "request",
        "source_snapshot",
        "worktree_fingerprint",
        "source",
        "artifact_root",
        "databases",
        "objects",
        "enabled_system_schedule_rows",
        "logical_inventory_sha256",
        "evidence",
        "catalog_sha256",
        "quiesced_at",
        "completed_at",
        "requires_offline_plan",
        "operator_declared_producers_only",
        "remote_writes",
        "clone_verified",
        "restore_qualified",
        "dump_semantic_digest_verified",
        "dump_validation",
    ];
    let root = exact(value, &fields, "源导出")?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("kind").and_then(Value::as_str) != Some("devex-clone-source-export")
        || root.get("status").and_then(Value::as_str) != Some("source_export_captured")
        || artifact_root != path.parent().unwrap_or(Path::new("."))
        || root.get("scope_id") != request["source"].get("scope_id")
        || root.get("id") != request.get("id")
        || root.get("worktree_fingerprint") != request.get("worktree_fingerprint")
        || root.get("source") != super::input::public_request(backend, request)?.get("source")
        || root.get("remote_writes").and_then(Value::as_u64) != Some(0)
        || root.get("clone_verified").and_then(Value::as_bool) != Some(false)
        || root.get("restore_qualified").and_then(Value::as_bool) != Some(false)
        || root
            .get("dump_semantic_digest_verified")
            .and_then(Value::as_bool)
            != Some(false)
        || root.get("requires_offline_plan").and_then(Value::as_bool) != Some(true)
        || root
            .get("operator_declared_producers_only")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("源导出类型、目录、scope 或实际验证状态不符".into());
    }
    let binding = root.get("request").ok_or("源导出缺少原请求绑定")?;
    if manifest::bound_file(backend, binding, "源导出原请求", true)? != request_path {
        return Err("源导出与外部原请求不同".into());
    }
    Ok(root)
}

/// 只建立已保存对象的权威输入窗口，不加载执行环境或复核其他阶段。
pub(crate) struct SavedObjectInputs {
    pub(crate) root: PathBuf,
    pub(crate) export: Value,
    pub(crate) request: Value,
    pub(crate) inventory: Value,
    held: Inputs,
}

impl SavedObjectInputs {
    pub(crate) fn load(backend: &Path, descriptor: &Value) -> Result<Self> {
        let path = manifest::bound_file(backend, descriptor, "已保存对象的原导出", true)?;
        if path.file_name().and_then(|name| name.to_str()) != Some("export.json") {
            return Err("已保存对象必须绑定正式 export.json".into());
        }
        let root = path.parent().ok_or("源导出缺少证据目录")?.to_owned();
        reject_failure(&root)?;
        let export = json_io::read(&path)?;
        let request_path = manifest::bound_file(backend, &export["request"], "源导出原请求", true)?;
        let inventory_path = super::evidence::bound_relative(
            &root,
            &export["evidence"]["inventory-before"],
            "inventory-before.json",
        )?;
        let public_request_path =
            super::evidence::bound_relative(&root, &export["evidence"]["request"], "request.json")?;
        let held = Inputs::open([
            path.clone(),
            request_path.clone(),
            inventory_path.clone(),
            public_request_path.clone(),
        ])?;
        manifest::bound_file(backend, descriptor, "已保存对象的原导出", true)?;
        if json_io::read(&path)? != export {
            return Err("已保存对象的导出收据在建立保护时变化".into());
        }
        let request = json_io::read(&request_path)?;
        super::input::validate_request(backend, &request)?;
        validate_shape(
            backend,
            &request,
            &request_path,
            &export,
            &path,
            Path::new(json_io::text(&export["artifact_root"], "源导出证据根")?),
        )?;
        if json_io::read(&public_request_path)? != super::input::public_request(backend, &request)?
        {
            return Err("源请求公开投影与已绑定的原请求不一致".into());
        }
        let inventory = json_io::read(&inventory_path)?;
        super::inventory_validate::objects(
            json_io::object(&inventory, "已绑定来源库存")?,
            json_io::text(&export["scope_id"], "来源 scope")?,
        )?;
        for (name, file) in [
            ("inventory-before", "inventory-before.json"),
            ("request", "request.json"),
        ] {
            super::evidence::bound_relative(&root, &export["evidence"][name], file)?;
        }
        Ok(Self {
            root,
            export,
            request,
            inventory,
            held,
        })
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.held.check()?;
        reject_failure(&self.root)
    }
}

fn reject_failure(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root.join("failure.json")) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("源证据包含失败标记，不能使用残留成功文件".into()),
    }
}

fn validate_evidence(
    context: &Context,
    root: &serde_json::Map<String, Value>,
    artifact_root: &Path,
) -> Result<()> {
    let evidence = root
        .get("evidence")
        .and_then(Value::as_object)
        .ok_or("源导出缺少证据")?;
    for name in [
        "request",
        "generation-before",
        "generation-after",
        "inventory-before",
        "inventory-after",
        "inventory-before-targets",
        "inventory-after-targets",
        "schema-before",
        "schema-after",
    ] {
        let binding = evidence.get(name).ok_or("源导出原始证据缺失")?;
        let binding = exact(binding, &["file", "bytes", "sha256"], "源导出原始证据")?;
        if binding.get("file").and_then(Value::as_str) != Some(format!("{name}.json").as_str()) {
            return Err("源原始证据名称与固定阶段不符".into());
        }
        let evidence_path = artifact_root.join(format!("{name}.json"));
        let actual = digest::file(&evidence_path)?;
        if actual.bytes != binding["bytes"].as_u64().unwrap_or_default()
            || actual.sha256 != binding["sha256"].as_str().unwrap_or_default()
        {
            return Err("源原始证据已变化".into());
        }
    }
    if json_io::read(&artifact_root.join("request.json"))?
        != super::input::public_request(&context.backend, &context.request)?
    {
        return Err("源请求公开投影与已绑定的私有原请求不一致".into());
    }
    let before_inventory = json_io::read(&artifact_root.join("inventory-before.json"))?;
    if digest::plan_hash(&inventory::logical(&before_inventory)?)?
        != root["logical_inventory_sha256"]
            .as_str()
            .unwrap_or_default()
    {
        return Err("源前后逻辑清单摘要不符".into());
    }
    let after_inventory = json_io::read(&artifact_root.join("inventory-after.json"))?;
    if inventory::logical(&before_inventory)? != inventory::logical(&after_inventory)? {
        return Err("源前后完整逻辑清单不一致".into());
    }
    Ok(())
}

fn validate_export_databases(
    context: &Context,
    root: &serde_json::Map<String, Value>,
    artifact_root: &Path,
    models: &super::database::Models,
) -> Result<()> {
    for database in root
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("源导出缺少数据库转储")?
    {
        let database = exact(database, &["key", "tables", "artifact"], "源数据库转储")?;
        let artifact = exact(
            database.get("artifact").ok_or("源转储缺少 artifact")?,
            &["file", "bytes", "sha256"],
            "源转储 artifact",
        )?;
        let file = artifact_root.join(
            artifact
                .get("file")
                .and_then(Value::as_str)
                .ok_or("源转储缺少文件")?,
        );
        let actual = digest::file(&file)?;
        if actual.bytes != artifact["bytes"].as_u64().unwrap_or_default()
            || actual.sha256 != artifact["sha256"].as_str().unwrap_or_default()
        {
            return Err("源转储文件摘要变化".into());
        }
        let key = database
            .get("key")
            .and_then(Value::as_str)
            .ok_or("源转储 key 无效")?;
        let db = context
            .databases()?
            .into_iter()
            .find(|item| item["key"].as_str() == Some(key))
            .ok_or("源转储指向未登记数据库")?;
        let catalog = super::super::sql::all_columns(
            &models.control,
            &models.tenant,
            db["kind"].as_str() == Some("combined"),
        );
        let counts = database
            .get("tables")
            .and_then(Value::as_object)
            .ok_or("源转储缺少表行数")?;
        let mut actual_counts = super::super::sql::empty_counts(&catalog);
        super::super::sql::for_each_row(&file, &catalog, None, |table, _| {
            *actual_counts.get_mut(table).ok_or("源转储包含未知表")? += 1;
            Ok(())
        })?;
        let declared = counts
            .iter()
            .map(|(key, value)| Ok((key.clone(), value.as_u64().ok_or("源转储行数无效")?)))
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        if actual_counts != declared {
            return Err("源转储实际行数与声明不同".into());
        }
    }
    Ok(())
}

fn validate_generation(
    context: &Context,
    root: &serde_json::Map<String, Value>,
    artifact_root: &Path,
) -> Result<()> {
    let current = proof::generation(context)?;
    let before_generation = json_io::read(&artifact_root.join("generation-before.json"))?;
    let after_generation = json_io::read(&artifact_root.join("generation-after.json"))?;
    if !generation_matches(&current, &before_generation) || before_generation != after_generation {
        return Err("源记录代次与当前源码、构建或运行收据不同".into());
    }
    if root.get("source_snapshot") != before_generation.get("source") {
        return Err("源快照与代次证据不同".into());
    }
    Ok(())
}

fn generation_matches(current: &Value, recorded: &Value) -> bool {
    [
        "source",
        "worktree_fingerprint",
        "runtime",
        "processes",
        "physical_binding",
        "maintenance",
        "request_sha256",
    ]
    .into_iter()
    .all(|field| current.get(field).is_some() && current.get(field) == recorded.get(field))
}

fn validate_export_objects(
    root: &serde_json::Map<String, Value>,
    artifact_root: &Path,
) -> Result<()> {
    let buckets = root
        .get("objects")
        .and_then(Value::as_array)
        .ok_or("源导出缺少对象桶")?;
    let expected = super::super::input::BUCKETS
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if buckets.len() != expected.len()
        || buckets
            .iter()
            .filter_map(|bucket| bucket.get("bucket").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>()
            != expected
    {
        return Err("源导出没有完整覆盖五个对象桶".into());
    }
    for bucket in buckets {
        let bucket = exact(bucket, &["bucket", "entries"], "源对象桶")?;
        for entry in bucket
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("源对象桶缺少条目")?
        {
            validate_object_entry(entry, artifact_root)?;
        }
    }
    Ok(())
}

fn validate_object_entry(entry: &Value, artifact_root: &Path) -> Result<()> {
    let entry = exact(
        entry,
        &[
            "key",
            "artifact",
            "metadata",
            "capture",
            "get_header_consistent",
        ],
        "源对象条目",
    )?;
    let artifact = exact(
        entry.get("artifact").unwrap(),
        &["file", "bytes", "sha256"],
        "源对象 artifact",
    )?;
    let capture = exact(
        entry.get("capture").unwrap(),
        &["file", "bytes", "sha256"],
        "源对象 capture",
    )?;
    let body = artifact_root.join(
        artifact
            .get("file")
            .and_then(Value::as_str)
            .ok_or("源对象缺少文件")?,
    );
    let capture_path = artifact_root.join(
        capture
            .get("file")
            .and_then(Value::as_str)
            .ok_or("源对象 capture 缺少文件")?,
    );
    let actual = digest::file(&body)?;
    if actual.bytes != artifact["bytes"].as_u64().unwrap_or_default()
        || actual.sha256 != artifact["sha256"].as_str().unwrap_or_default()
        || capture_path.file_name().and_then(|value| value.to_str()) != Some("capture.json")
    {
        return Err("源对象字节或采集收据路径变化".into());
    }
    let capture_value = json_io::read(&capture_path)?;
    if capture_value.get("artifact")
        != Some(&json!({
            "file": "object.bin",
            "bytes": actual.bytes,
            "sha256": actual.sha256,
        }))
    {
        return Err("源对象 capture 与实体字节不一致".into());
    }
    let capture_digest = digest::file(&capture_path)?;
    if capture_digest.bytes != capture["bytes"].as_u64().unwrap_or_default()
        || capture_digest.sha256 != capture["sha256"].as_str().unwrap_or_default()
    {
        return Err("源对象 capture 摘要变化".into());
    }
    Ok(())
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    #[test]
    fn bound_source_cannot_accept_a_reduced_or_changed_physical_identity() {
        let current = json!({
            "source":{"head":"a".repeat(40)}, "worktree_fingerprint":"fixture",
            "runtime":{"scope_id":"fixture"}, "processes":{}, "maintenance":{},
            "request_sha256":"b".repeat(64),
            "physical_binding":{"format_version":1,"scope_id":"fixture",
                "databases":[{"key":"shared-control","kind":"combined","mode":"shared",
                    "server_uuid":"server-a","host":"127.0.0.1","port":3306,
                    "database":"fixture","tls_mode":"required"}],
                "s3":{"endpoint":"http://127.0.0.1:29000","region":"us-east-1"},
                "sha256":"c".repeat(64)}
        });
        assert!(generation_matches(&current, &current));
        let mut reduced = current.clone();
        reduced["physical_binding"] = json!({"scope_id":"fixture",
            "object_endpoint":"http://127.0.0.1:29000",
            "databases":[{"key":"shared-control","kind":"combined",
                "server_uuid":"server-a","database":"fixture"}]});
        assert!(!generation_matches(&current, &reduced));
        for pointer in [
            "/physical_binding/databases/0/server_uuid",
            "/physical_binding/databases/0/host",
            "/physical_binding/databases/0/mode",
            "/physical_binding/databases/0/tls_mode",
            "/physical_binding/s3/endpoint",
            "/physical_binding/sha256",
        ] {
            let mut changed = current.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("changed");
            assert!(!generation_matches(&current, &changed), "{pointer}");
        }
        let mut missing = current.clone();
        missing.as_object_mut().unwrap().remove("physical_binding");
        assert!(!generation_matches(&current, &missing));
    }
}

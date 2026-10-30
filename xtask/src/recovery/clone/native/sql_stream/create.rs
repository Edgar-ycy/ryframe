use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use super::{
    super::{digest, json as json_io, sql},
    binding, model,
};
use crate::{Result, recovery::monitoring::execution::ProtectedInputs};
use serde_json::{Value, json};

pub(super) fn run(args: &Value) -> Result<Value> {
    model::exact(
        args,
        &["backend", "path", "expected", "catalog", "only_table"],
    )?;
    model::exact(&args["expected"], &["bytes", "sha256"])?;
    model::sha(&args["expected"]["sha256"])?;
    if args["expected"]["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes == 0)
    {
        return Err("SQL 源字节数无效".into());
    }
    let backend = fs::canonicalize(model::path(&args["backend"], true)?)?;
    let source = model::path(&args["path"], false)?;
    let catalog = model::catalog(&args["catalog"])?;
    let selected = if args["only_table"].is_null() {
        None
    } else {
        Some(model::text(&args["only_table"])?)
    };
    if selected.is_some_and(|table| !catalog.contains_key(table)) {
        return Err("SQL 流过滤表未登记".into());
    }
    let guard = ProtectedInputs::open([source.clone()])?;
    let source_binding = binding::receipt(&source, model::text(&args["path"])?, false)?;
    if source_binding["bytes"] != args["expected"]["bytes"]
        || source_binding["sha256"] != args["expected"]["sha256"]
    {
        return Err("SQL 源内容与原绑定不同".into());
    }
    let file = binding::open(&source, &source_binding)?;
    let parent = model::owned_parent(&backend)?;
    let root = tempfile::Builder::new()
        .prefix("sql-rows-")
        .tempdir_in(parent)?
        .keep();
    let root = fs::canonicalize(root)?;
    let result = create(
        &root,
        &backend,
        source_binding,
        file,
        &catalog,
        selected,
        &guard,
    );
    guard.finish(result).map_err(|error| {
        let code = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error);
        format!(
            "SQL 行流创建失败；原因={error}；本地 OS {code:?}；结果保留在 {}；未返回 ready，不自动重试或清理",
            root.display()
        )
        .into()
    })
}

fn create(
    root: &Path,
    backend: &Path,
    source_binding: Value,
    file: File,
    catalog: &super::super::catalog::Catalog,
    selected: Option<&str>,
    guard: &ProtectedInputs,
) -> Result<Value> {
    let mut descriptor = json!({"format_version": 1, "kind": model::KIND,
        "backend": backend, "root": root, "source": source_binding,
        "catalog_sha256": digest::plan_hash(&serde_json::to_value(catalog)?)?, "only_table": selected,
        "tables": sql::empty_counts(catalog), "source_rows": 0, "rows": 0, "artifact": Value::Null});
    let manifest = root.join("manifest.json");
    let mut preparing = descriptor.clone();
    preparing["state"] = json!("preparing");
    json_io::write_new(&manifest, &preparing)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let output_path = root.join("rows.jsonl");
    let mut output = BufWriter::new(binding::HashingIo::new(options.open(&output_path)?));
    let hashing = binding::HashingIo::new(file);
    let mut reader = BufReader::new(hashing);
    let mut counts = sql::empty_counts(catalog);
    let mut rows = 0_u64;
    sql::for_each_reader(&mut reader, catalog, None, |table, row| {
        let count = counts.get_mut(table).ok_or("SQL 流表计数缺失")?;
        *count = count.checked_add(1).ok_or("SQL 流表计数溢出")?;
        if selected.is_none_or(|selected| selected == table) {
            serde_json::to_writer(&mut output, &wire(table, row))?;
            output.write_all(b"\n")?;
            rows = rows.checked_add(1).ok_or("SQL 流行数溢出")?;
        }
        Ok(())
    })?;
    let (file, consumed) = reader.into_inner().finish();
    if consumed["bytes"] != descriptor["source"]["bytes"]
        || consumed["sha256"] != descriptor["source"]["sha256"]
    {
        return Err("SQL 流实际消费字节与源绑定不同".into());
    }
    binding::check_handle(
        Path::new(model::text(&descriptor["source"]["path"])?),
        &file,
        &descriptor["source"],
    )?;
    guard.assert_unchanged()?;
    let artifact = binding::finish_output(output, &output_path)?;
    descriptor["source_rows"] = json!(
        counts
            .values()
            .try_fold(0_u64, |sum, count| sum.checked_add(*count))
            .ok_or("SQL 流总行数溢出")?
    );
    descriptor["tables"] = serde_json::to_value(counts)?;
    descriptor["rows"] = json!(rows);
    descriptor["artifact"] = artifact;
    publish(root, descriptor, guard)
}

fn publish(root: &Path, mut descriptor: Value, source_guard: &ProtectedInputs) -> Result<Value> {
    let artifact = root.join("rows.jsonl");
    let artifact_guard = ProtectedInputs::open([artifact.clone()])?;
    if binding::receipt(&artifact, "rows.jsonl", true)? != descriptor["artifact"] {
        return Err("SQL 流结果在发布前变化".into());
    }
    source_guard.assert_unchanged()?;
    let mut ready = descriptor.clone();
    ready["state"] = json!("ready");
    if serde_json::to_vec(&ready)?.len() as u64 > json_io::MAX_JSON_BYTES - 4096 {
        return Err("SQL 流 ready 元数据超过原协议限制".into());
    }
    json_io::write_replace(&root.join("manifest.json"), &ready)?;
    descriptor["manifest"] = binding::receipt(&root.join("manifest.json"), "manifest.json", true)?;
    let manifest_guard = ProtectedInputs::open([root.join("manifest.json")])?;
    model::descriptor(&descriptor)?;
    source_guard.assert_unchanged()?;
    artifact_guard.finish(manifest_guard.finish(Ok(descriptor)))
}

fn wire(table: &str, row: &sql::Row) -> Value {
    let row = row
        .iter()
        .map(|(name, value)| {
            let cell = match value {
                sql::SqlValue::Null => json!({"kind": "null"}),
                sql::SqlValue::Number(token) => json!({"kind": "number", "value": token}),
                sql::SqlValue::Bytes(bytes) => {
                    json!({"kind": "bytes", "value": hex::encode(bytes)})
                }
                sql::SqlValue::Text(text) => json!({"kind": "text", "value": text}),
                sql::SqlValue::Json(value) => json!({"kind": "json", "value": value}),
            };
            (name.clone(), cell)
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"table": table, "row": row})
}

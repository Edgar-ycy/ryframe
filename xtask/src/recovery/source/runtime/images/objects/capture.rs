//! 按原 ownership→HEAD→条件 GET→HEAD→ownership 顺序采集，不重试对象读取。
use crate::{
    Result,
    recovery::clone::native::{
        digest,
        export::{
            input::Context,
            object::{ObjectCall, object_read},
            object_owner::owner_proofs,
            transport::{StdoutTarget, Transport},
        },
        json as document,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub(in crate::recovery::source::evidence::images) fn run<T: Transport + Sync>(
    context: &Context,
    transport: &T,
    inventory: &Value,
) -> Result<Vec<Value>> {
    let root = context.output().join("objects");
    fs::create_dir(&root)?;
    let mut buckets = document::array(&inventory["objects"], "来源完整五桶")?
        .iter()
        .collect::<Vec<_>>();
    buckets.sort_by_key(|row| row["bucket"].as_str());
    let mut tasks = Vec::new();
    let max = context.request["max_object_bytes"]
        .as_u64()
        .filter(|bytes| *bytes > 0)
        .ok_or("来源对象上限无效")?;
    for entry in buckets {
        let bucket = document::text(&entry["bucket"], "来源桶名")?;
        let mut entries = document::array(&entry["entries"], "来源完整对象")?
            .iter()
            .collect::<Vec<_>>();
        entries.sort_by_key(|row| row["key"].as_str());
        for expected in entries {
            let key = document::text(&expected["key"], "来源对象键")?;
            tasks.push((
                bucket,
                key,
                json!({"bytes":expected["bytes"],"sha256":expected["sha256"]}),
            ));
        }
    }
    let mut descriptors = Vec::new();
    for batch in tasks.chunks(4) {
        let completed = std::thread::scope(|scope| {
            let workers = batch
                .iter()
                .map(|task| {
                    let root = &root;
                    scope.spawn(move || {
                        capture_one(context, transport, root, task, max)
                            .map_err(|error| error.to_string())
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .map_err(|_| "来源对象读取线程异常；保留本批现场，禁止重试".to_owned())?
                })
                .collect::<std::result::Result<Vec<_>, String>>()
        })?;
        descriptors.extend(completed);
    }
    Ok(descriptors)
}

fn capture_one<T: Transport>(
    context: &Context,
    transport: &T,
    root: &Path,
    task: &(&str, &str, Value),
    max: u64,
) -> Result<Value> {
    let (bucket, key, expected) = task;
    let token = hex::encode(Sha256::digest(format!("{bucket}:{key}").as_bytes()));
    let directory = root.join(token);
    fs::create_dir(&directory)?;
    one(context, transport, bucket, key, &directory, expected, max)?;
    crate::recovery::clone::native::resources::common::binding(&directory.join("capture.json"))
}

fn one<T: Transport>(
    context: &Context,
    transport: &T,
    bucket: &str,
    key: &str,
    directory: &Path,
    expected: &Value,
    max: u64,
) -> Result<()> {
    let request = json!({"format_version":1,"kind":"devex-clone-object-capture","side":"source",
        "scope_id":context.source()["scope_id"],"endpoint":context.object_identity["endpoint"],"region":context.object_identity["region"],
        "bucket":bucket,"key":key,"expected":expected,"max_bytes":max,"transport":"rust-s3-sigv4","clone_verified":false});
    document::write_new(&directory.join("intent.json"), &request)?;
    let result = (|| {
        let (before, bindings) = initial_ownership(context, transport, directory)?;
        let head = read(
            context,
            transport,
            directory,
            ObjectCall {
                operation: "head-object",
                bucket,
                key,
                stdout: StdoutTarget::Capture,
                extra: Some(&[]),
                label: "head-before",
                max_bytes: max,
            },
        )?;
        let conditions = conditions(&head, expected, max)?;
        let body = directory.join("object.bin");
        let get = read(
            context,
            transport,
            directory,
            ObjectCall {
                operation: "get-object",
                bucket,
                key,
                stdout: StdoutTarget::File(body.clone()),
                extra: Some(&conditions),
                label: "get",
                max_bytes: max,
            },
        )?;
        let repeated = read(
            context,
            transport,
            directory,
            ObjectCall {
                operation: "head-object",
                bucket,
                key,
                stdout: StdoutTarget::Capture,
                extra: Some(&conditions),
                label: "head-after",
                max_bytes: max,
            },
        )?;
        let after = owner_proofs(context, transport, directory, "owner-after")?;
        let actual = digest::file(&body)?;
        let value = super::result(
            &request,
            &head,
            &get,
            &repeated,
            &json!({"bytes":actual.bytes,"sha256":actual.sha256}),
            before,
            after,
        )?;
        verify_bindings(directory, &bindings)?;
        document::write_new(&directory.join("capture.json"), &value)?;
        super::verify(&context.backend, context.source(), directory, expected, max)?;
        Ok(())
    })();
    retain_failure(directory, &request, result)
}

fn conditions(head: &Value, expected: &Value, max: u64) -> Result<Vec<String>> {
    let identity = super::inspect(head)?;
    if identity["identity"]["ContentLength"] != expected["bytes"]
        || expected["bytes"].as_u64().is_none_or(|bytes| bytes > max)
    {
        return Err("对象 HEAD 大小与登记值或上限不符".into());
    }
    let mut conditions = vec![
        "--if-match".into(),
        document::text(&identity["identity"]["ETag"], "对象 ETag")?.to_owned(),
    ];
    if let Some(version) = identity["identity"]["VersionId"].as_str() {
        conditions.extend(["--version-id".into(), version.into()]);
    }
    Ok(conditions)
}

fn verify_bindings(directory: &Path, bindings: &Value) -> Result<()> {
    for row in document::array(bindings, "对象读取起始证据")? {
        if row["type"] == "file" {
            let path = directory.join(document::text(&row["path"], "对象起始证据路径")?);
            let digest = digest::file(&path)?;
            if row["bytes"] != digest.bytes || row["sha256"] != digest.sha256 {
                return Err("对象读取期间起始证据发生变化".into());
            }
        }
    }
    Ok(())
}

fn retain_failure(directory: &Path, request: &Value, result: Result<()>) -> Result<()> {
    let Err(error) = result else {
        return Ok(());
    };
    let publication = document::write_new(
        &directory.join("failure.json"),
        &json!({"status":"capture_failed","request":request,
        "reason":"对象只读采集未完成；保留全部证据并核对原始绑定，不自动重试"}),
    );
    match publication {
        Ok(()) => Err(error),
        Err(publication) => Err(format!(
            "对象采集失败：{error}；失败证据发布也未完成：{publication}；保留现场，不重试"
        )
        .into()),
    }
}

fn read<T: Transport>(
    context: &Context,
    transport: &T,
    directory: &Path,
    call: ObjectCall<'_>,
) -> Result<Value> {
    let operation = call.operation;
    let stage = call.label;
    let value = object_read(context, transport, call)?;
    document::write_new(&directory.join(format!("{stage}.json")), &value)?;
    document::write_new(
        &directory.join(format!("{stage}.diagnostic.json")),
        &json!({"operation":operation,"returncode":0,"error_type":null,"stderr":""}),
    )?;
    Ok(value)
}

fn initial_ownership<T: Transport>(
    context: &Context,
    transport: &T,
    directory: &Path,
) -> Result<(Vec<Value>, Value)> {
    let before = owner_proofs(context, transport, directory, "owner-before")?;
    let bindings = crate::recovery::clone::native::target_verify::tree::snapshot(
        &context.backend,
        directory,
        false,
    )?;
    Ok((before, bindings))
}

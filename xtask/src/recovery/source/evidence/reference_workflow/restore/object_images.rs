//! 参考恢复的五桶完整观察和逐对象真实字节证明。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{
            copy_transfer::{
                images::ObjectObservation,
                observe_object::{self, ObserveTarget, Observed},
                observe_source::ReadWindow,
                verify_target_objects::{self, VerifyTarget},
            },
            export::transport::{CommandOutput, CommandSpec, Transport},
            json as document,
            resources::common,
            s3::{Request, app_configuration, protocol::validate_key},
            target_verify::{fresh::initialized_objects, storage::StorageInputs},
        },
        inputs::{document as binding, domain::BUCKETS},
    },
};

pub(super) struct Observe<'a> {
    pub(super) storage: &'a StorageInputs<'a>,
    pub(super) configuration_root: &'a Path,
    pub(super) configuration: &'a Value,
    pub(super) directory: &'a Path,
    pub(super) scope: &'a str,
    pub(super) endpoint: &'a str,
    pub(super) region: &'a str,
    pub(super) manifest: &'a Value,
    pub(super) expected: &'a Value,
    pub(super) plan_sha256: &'a str,
    pub(super) generation_sha256: &'a str,
}

/// 目录由父控制器创建，checkpoint 必须可供原生最终 HEAD 的各个线程使用。
pub(super) fn capture<T: Transport + Sync>(
    request: &Observe<'_>,
    after: bool,
    transport: &T,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
) -> Result<(Value, Vec<Value>)> {
    checkpoint()?;
    request.validate()?;
    let result = if after {
        capture_after(request, transport, checkpoint)
    } else {
        capture_before(request, transport, checkpoint)
    };
    checked(result, checkpoint())
}

impl Observe<'_> {
    fn validate(&self) -> Result<()> {
        let backend = self.storage.backend;
        common::local_directory(backend, self.directory, "参考恢复对象观察目录")?;
        common::scope_id(self.scope)?;
        document::exact(self.expected, &BUCKETS, "参考恢复预期对象像")?;
        let target = document::object(&self.storage.request["target"], "参考恢复原目标")?;
        if self.storage.target_scope()? != self.scope
            || target["s3"]["endpoint"] != self.endpoint
            || target["s3"]["region"] != self.region
        {
            return Err("参考恢复对象观察与原目标 scope、端点或区域不同".into());
        }
        let path = Path::new(document::text(
            &self.storage.request["config_dir"],
            "参考恢复原目标配置路径",
        )?);
        if app_configuration::configuration_value(self.configuration_root, path, "test")?
            != *self.configuration
            || app_configuration::private_configuration(
                self.configuration_root,
                self.configuration,
            )? != target["s3"]
        {
            return Err("参考恢复对象观察未绑定原目标 test 配置".into());
        }
        Ok(())
    }
}

fn capture_before<T: Transport>(
    request: &Observe<'_>,
    transport: &T,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<(Value, Vec<Value>)> {
    let inputs = StorageInputs {
        backend: request.storage.backend,
        request: request.storage.request,
        environment: request.storage.environment,
        observation_dir: request.directory,
    };
    let image = initialized_objects::observe(
        &inputs,
        &CheckedTransport {
            transport,
            checkpoint,
        },
    )?;
    if image != *request.expected {
        return Err("参考恢复五桶前像与初始化证据不同".into());
    }
    Ok((image, Vec::new()))
}

struct CheckedTransport<'a, T> {
    transport: &'a T,
    checkpoint: &'a dyn Fn() -> Result<()>,
}

impl<T: Transport> Transport for CheckedTransport<'_, T> {
    fn run(&self, _: CommandSpec) -> Result<CommandOutput> {
        Err("参考恢复对象观察只接受原生 S3 读取".into())
    }

    fn s3(&self, request: Request) -> Result<Value> {
        (self.checkpoint)()?;
        checked(self.transport.s3(request), (self.checkpoint)())
    }
}

fn checked<T>(result: Result<T>, checkpoint: Result<()>) -> Result<T> {
    match (result, checkpoint) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(check)) => {
            Err(format!("{primary}；参考恢复对象观察复核同时失败：{check}").into())
        }
    }
}

fn capture_after<T: Transport + Sync>(
    request: &Observe<'_>,
    transport: &T,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
) -> Result<(Value, Vec<Value>)> {
    let backend = request.storage.backend;
    binding::digest(&json!(request.plan_sha256), "参考恢复计划摘要")?;
    binding::digest(&json!(request.generation_sha256), "参考恢复代次摘要")?;
    let entries = entries(request)?;
    let reads = ReadWindow::new(
        backend,
        request.configuration_root,
        request.directory,
        request.scope,
        request.configuration,
        transport,
        checkpoint,
    )?;
    let image = bucket_image(&reads, "before")?;
    if image != *request.expected {
        return Err("参考恢复后五桶完整范围或 ownership 不符".into());
    }
    let captures = capture_entries(request, entries, transport, checkpoint)?;
    let directory = request.directory.join("object-heads");
    let verified = verify_target_objects::verify(
        VerifyTarget {
            backend,
            configuration_root: request.configuration_root,
            work: request.directory,
            directory: &directory,
            plan_sha256: request.plan_sha256,
            generation_sha256: request.generation_sha256,
            scope: request.scope,
            endpoint: request.endpoint,
            region: request.region,
            configuration: request.configuration,
            items: &captures.items,
            observations: &captures.observations,
        },
        transport,
        checkpoint,
    )?;
    let verified_file = common::binding(&directory.join("verified.json"))?;
    if common::read_bound(backend, &verified_file, "参考恢复最终对象 HEAD 证明")? != verified
        || bucket_image(&reads, "after")? != image
    {
        return Err("参考恢复对象最终 HEAD、五桶完整范围或 ownership 变化".into());
    }
    let mut evidence = captures.evidence;
    evidence.push(verified_file);
    for proof in &evidence {
        common::bound_file(backend, proof, "参考恢复对象证明")?;
    }
    Ok((image, evidence))
}

fn bucket_image<T: Transport>(reads: &ReadWindow<'_, T>, phase: &str) -> Result<Value> {
    let mut buckets = BUCKETS;
    buckets.sort_unstable();
    let owners = reads.owners(&buckets, &format!("owners-{phase}"))?;
    let mut image = BTreeMap::new();
    for (bucket, owner) in buckets.into_iter().zip(owners) {
        if owner["bucket"] != bucket {
            return Err("参考恢复对象 owner 不属于原登记桶".into());
        }
        image.insert(
            bucket,
            json!({"keys":reads.keys(bucket, phase)?,
            "owner":{"bytes":owner["bytes"],"sha256":owner["sha256"]}}),
        );
    }
    Ok(json!(image))
}

struct Entry<'a> {
    bucket: &'a str,
    index: usize,
    original: &'a str,
    key: String,
    expected: Value,
    bytes: u64,
}

fn entries<'a>(request: &'a Observe<'_>) -> Result<Vec<Entry<'a>>> {
    let source = document::text(&request.manifest["scope_id"], "参考恢复备份 scope")?;
    common::scope_id(source)?;
    if source == request.scope {
        return Err("参考恢复源与目标对象 scope 重叠".into());
    }
    let prefix = format!("{source}/");
    let owner = format!("{}/.ryframe-owner", request.scope);
    let mut buckets = BTreeSet::new();
    let mut result = Vec::new();
    for row in document::array(&request.manifest["objects"], "参考恢复备份对象桶")? {
        let bucket = document::text(&row["bucket"], "参考恢复备份对象桶")?;
        if !BUCKETS.contains(&bucket) || !buckets.insert(bucket) || row["prefix"] != prefix {
            return Err("参考恢复备份对象桶未登记、重复或不属于原 scope".into());
        }
        let mut keys = BTreeSet::from([owner.clone()]);
        for (index, item) in document::array(&row["entries"], "参考恢复备份对象")?
            .iter()
            .enumerate()
        {
            let entry = entry(source, &prefix, request.scope, bucket, index, item)?;
            if !keys.insert(entry.key.clone()) {
                return Err("参考恢复备份对象 key 重复".into());
            }
            result.push(entry);
        }
        if request.expected[bucket]["keys"] != json!(keys) {
            return Err("参考恢复对象计划与预期五桶键集合不同".into());
        }
    }
    if buckets.len() != BUCKETS.len() {
        return Err("参考恢复备份必须完整覆盖五桶".into());
    }
    Ok(result)
}

fn entry<'a>(
    source: &str,
    prefix: &str,
    target: &str,
    bucket: &'a str,
    index: usize,
    item: &'a Value,
) -> Result<Entry<'a>> {
    let original = document::text(&item["key"], "原备份对象键")?;
    validate_key(source, original, false)?;
    let suffix = original
        .strip_prefix(prefix)
        .ok_or("备份对象不属于原 scope")?;
    let key = format!("{target}/{suffix}");
    validate_key(target, &key, false)?;
    let bytes = item["bytes"]
        .as_u64()
        .filter(|bytes| *bytes > 0)
        .ok_or("原备份对象大小无效")?;
    let sha256 = binding::digest(&item["sha256"], "原备份对象摘要")?;
    let expected = json!({"bytes":bytes,"sha256":sha256});
    Ok(Entry {
        bucket,
        index,
        original,
        key,
        expected,
        bytes,
    })
}

#[derive(Default)]
struct Captures {
    items: Vec<Value>,
    evidence: Vec<Value>,
    observations: BTreeMap<(String, String), Observed>,
}

fn capture_entries<T: Transport>(
    request: &Observe<'_>,
    entries: Vec<Entry<'_>>,
    transport: &T,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Captures> {
    let backend = request.storage.backend;
    let mut captures = Captures::default();
    for entry in entries {
        let directory = request
            .directory
            .join(format!("object-{}-{}", entry.bucket, entry.index));
        let observed = observe_object::observe(
            ObserveTarget {
                backend,
                configuration_root: request.configuration_root,
                work: request.directory,
                directory: &directory,
                plan_sha256: request.plan_sha256,
                scope: request.scope,
                endpoint: request.endpoint,
                region: request.region,
                configuration: request.configuration,
                bucket: entry.bucket,
                key: &entry.key,
                expected: &entry.expected,
                max_bytes: entry.bytes,
            },
            transport,
            checkpoint,
        )?;
        let ObjectObservation::Captured(capture) = observed.image_evidence() else {
            return Err("参考恢复后对象缺少真实完整字节观察".into());
        };
        if observed.receipt()["status"] != "object_present"
            || common::read_bound(backend, observed.descriptor(), "参考恢复对象观察证明")?
                != *observed.receipt()
        {
            return Err("参考恢复后对象原观察收据不一致".into());
        }
        captures
            .items
            .push(json!({"bucket":entry.bucket,"source_key":entry.original,
            "target_key":entry.key,"artifact":{"file":"object.bin","bytes":entry.bytes,
            "sha256":entry.expected["sha256"]},"metadata":capture["metadata"]}));
        captures.evidence.push(observed.descriptor().clone());
        captures
            .observations
            .insert((entry.bucket.to_owned(), entry.key), observed);
    }
    Ok(captures)
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/object_images_tests.rs"]
mod tests;

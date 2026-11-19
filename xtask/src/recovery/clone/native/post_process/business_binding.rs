//! 复制后业务验证只消费原登记输入，补查现有生产者未读取的目标对象。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use super::super::{
    copy_transfer::{
        images::ObjectObservation,
        observe_object::{self, ObserveTarget},
        observe_source::ReadWindow,
    },
    digest,
    export::transport::Transport,
    input::{BUCKETS, digest_shape, name},
    json as document,
    resources::common,
    s3::{app_configuration, protocol::validate_key},
    target_verify::{guard::Inputs, publication},
};
use crate::{
    Result,
    recovery::inputs::{document as proof, domain},
};

const ORIGINALS: [&str; 5] = [
    "reference_plan",
    "dataset",
    "copy_result",
    "copy_stage_receipt",
    "ledger_head",
];
const COPY: [&str; 4] = [
    "plan_sha256",
    "generation_sha256",
    "source_export_sha256",
    "fresh_target_sha256",
];

type Key = (String, String);

pub(super) struct Business<'a> {
    backend: &'a Path,
    output: &'a Path,
    output_identity: Value,
    target: Value,
    target_sha256: String,
    input_files: Value,
    originals: Inputs,
    target_input: Inputs,
    objects: Objects<'a>,
}

pub(super) struct ObjectRead<'a> {
    pub(super) configuration_root: &'a Path,
    pub(super) configuration: &'a Value,
    pub(super) endpoint: &'a str,
    pub(super) region: &'a str,
}

pub(super) struct Completion {
    pub(super) evidence: Value,
    pub(super) objects: Value,
    directories: Vec<Value>,
}

impl Completion {
    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        for identity in &self.directories {
            let directory = common::directory_identity(backend, identity)?;
            observe_object::reject_failure(&directory)?;
        }
        Ok(())
    }
}

fn observation_directory_identity(backend: &Path, path: &Path) -> Result<Value> {
    let directory = common::local_directory(backend, path, "复制后对象观察目录")?;
    let (device, inode) = common::device_inode_for(&directory, &fs::symlink_metadata(&directory)?)?;
    Ok(json!({"path":directory,"device":device,"inode":inode}))
}

pub(super) fn prepare<'a>(
    backend: &'a Path,
    output: &'a Path,
    request: &Value,
    copy_plan: &'a Value,
    selected: &Value,
) -> Result<Business<'a>> {
    common::local_directory(backend, output, "复制后业务证据目录")?;
    let (device, inode) = common::device_inode_for(output, &fs::symlink_metadata(output)?)?;
    let output_identity = json!({"path":output,"device":device,"inode":inode});
    let originals = protect_originals(backend, request)?;
    let read = |key| proof::bound_file(backend, &request[key], "复制后原绑定输入");
    let (reference_binding, reference) = read("reference_plan")?;
    let (dataset_binding, dataset) = read("dataset")?;
    let (_, copy_result) = read("copy_result")?;
    let (stage_binding, stage) = read("copy_stage_receipt")?;
    let (ledger_binding, ledger) = read("ledger_head")?;
    let copy = copy_binding(
        copy_plan,
        &copy_result.value,
        &stage.value,
        &ledger.value,
        &stage_binding.sha256,
        &ledger_binding.sha256,
    )?;
    let scope = name(document::text(&selected["scope_id"], "复制业务目标 scope")?)?;
    let source = name(document::text(
        &reference.value["source"]["scope_id"],
        "原业务来源 scope",
    )?)?;
    let reference_sha = digest::plan_hash(&reference.value)?;
    if dataset.value["format_version"] != 1
        || dataset.value["plan_sha256"] != reference_sha
        || dataset.value["source_scope_id"] != source
        || copy_plan["target_scope"] != scope
        || scope == source
    {
        return Err("原参考计划、业务数据集与当前复制目标绑定不同".into());
    }
    for field in ["api_url", "frontend_url"] {
        let endpoint = domain::endpoint(&selected[field], "复制业务本机根地址", None)?;
        if !matches!(endpoint.path.as_str(), "" | "/") {
            return Err("复制业务 API 与前端必须使用本机根地址".into());
        }
    }
    let objects = Objects::bind(copy_plan, &dataset.value, scope)?;
    let target = json!({"format_version":1,"kind":"devex-copy-business-target",
        "source_plan_sha256":reference_sha,"source_scope_id":source,
        "target":{"scope_id":scope,"api_url":selected["api_url"],"frontend_url":selected["frontend_url"]},
        "copy":copy});
    let target_sha256 = digest::plan_hash(&target)?;
    originals.check()?;
    common::directory_identity(backend, &output_identity)?;
    let target_path = output.join("business-target.json");
    publication::publish(&target_path, &target)?;
    let target_input = Inputs::open([target_path.clone()])?;
    let raw = proof::read(&target_path, backend, "复制业务目标绑定")?;
    if raw.value != target {
        return Err("复制业务目标绑定在发布期间变化".into());
    }
    let input_files = json!({"plan_sha256":reference_binding.sha256,
        "dataset_sha256":dataset_binding.sha256,"target_binding_sha256":raw.sha256()});
    let business = Business {
        backend,
        output,
        output_identity,
        target,
        target_sha256,
        input_files,
        originals,
        target_input,
        objects,
    };
    business.check()?;
    Ok(business)
}

fn protect_originals(backend: &Path, request: &Value) -> Result<Inputs> {
    let mut files = Vec::with_capacity(ORIGINALS.len());
    for key in ORIGINALS {
        let binding = proof::binding_shape(&request[key], "复制后原文件")?;
        files.push(common::common_file(backend, &binding.path, "复制后原文件")?);
    }
    Inputs::open(files)
}

fn copy_binding(
    plan: &Value,
    result: &Value,
    stage: &Value,
    ledger: &Value,
    stage_sha256: &str,
    ledger_sha256: &str,
) -> Result<Value> {
    let mut outer = document::object(stage, "原复制外层结果")?.clone();
    outer.remove("continuation_receipt");
    if Value::Object(outer) != *result
        || result["status"] != "data_steps_verified"
        || result["plan_sha256"] != plan["plan_sha256"]
        || ledger["format_version"] != 1
        || ledger["plan_sha256"] != result["plan_sha256"]
        || ledger["generation_sha256"] != result["generation_sha256"]
    {
        return Err("复制业务绑定不属于原计划、外层结果及同代账本".into());
    }
    let mut fields = serde_json::Map::new();
    for key in COPY {
        fields.insert(key.into(), json!(digest_shape(&result[key])?));
    }
    fields.insert("stage_receipt_sha256".into(), json!(stage_sha256));
    fields.insert("ledger_head_sha256".into(), json!(ledger_sha256));
    Ok(Value::Object(fields))
}

impl Business<'_> {
    pub(super) fn check(&self) -> Result<()> {
        common::directory_identity(self.backend, &self.output_identity)?;
        self.originals.check()?;
        self.target_input.check()
    }

    pub(super) fn finish<T: Transport>(
        &self,
        observed: &Value,
        access: ObjectRead<'_>,
        transport: &T,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Completion> {
        checkpoint()?;
        self.check()?;
        self.verify_result(observed)?;
        let path = self.output.join("business-result.json");
        publication::publish(&path, observed)?;
        let result_input = Inputs::open([path.clone()])?;
        let evidence = common::binding(&path)?;
        if common::read_bound(self.backend, &evidence, "复制后业务原生结果")? != *observed
        {
            return Err("复制后业务结果在发布期间变化".into());
        }
        let directories = self.verify_objects(access, transport, checkpoint)?;
        checkpoint()?;
        self.check()?;
        result_input.check()?;
        Ok(Completion {
            evidence,
            objects: json!({
                "business_objects":self.objects.covered.len(),
                "additional_objects":self.objects.plan.len()-self.objects.covered.len(),
                "all_scoped_keys_verified":true,"additional_objects_downloaded":true,
            }),
            directories,
        })
    }

    fn verify_result(&self, observed: &Value) -> Result<()> {
        document::exact(
            observed,
            &[
                "format_version",
                "status",
                "side",
                "scope_id",
                "plan_sha256",
                "source_scope_id",
                "target_binding_sha256",
                "copy",
                "actions",
                "clone_verified",
                "restore_success",
                "tenants",
                "posts",
                "files",
                "input_files",
            ],
            "复制业务原生生产者结果",
        )?;
        if observed["format_version"] != 1
            || observed["status"] != "copy_existing_data_verified"
            || observed["side"] != "copy_target"
            || observed["scope_id"] != self.target["target"]["scope_id"]
            || observed["plan_sha256"] != self.target["source_plan_sha256"]
            || observed["source_scope_id"] != self.target["source_scope_id"]
            || observed["target_binding_sha256"] != self.target_sha256
            || observed["copy"] != self.target["copy"]
            || observed["input_files"] != self.input_files
            || observed["actions"]
                != json!({"business":"read_only","objects":"read_only","session":"login_logout"})
            || observed["clone_verified"] != false
            || observed["restore_success"] != false
            || observed["tenants"].as_u64() != Some(self.objects.tenants)
            || observed["posts"].as_u64() != Some(self.objects.posts)
            || observed["files"].as_u64() != Some(self.objects.covered.len() as u64)
        {
            return Err("业务读取结果没有绑定原计划、数据集、复制目标或完整业务样本".into());
        }
        Ok(())
    }

    fn verify_objects<T: Transport>(
        &self,
        access: ObjectRead<'_>,
        transport: &T,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Vec<Value>> {
        let scope = document::text(&self.target["target"]["scope_id"], "复制业务目标 scope")?;
        if access.configuration["app_env"] != "test"
            || app_configuration::public_configuration(
                access.configuration_root,
                access.configuration,
            )? != json!({"endpoint":access.endpoint,"region":access.region})
        {
            return Err("复制后对象核验须绑定原目标执行根及 test 端点".into());
        }
        let directory = self.output.join("object-scope");
        common::ensure_new_directory(self.backend, &directory, "复制后五桶观察目录")?;
        let mut directories = vec![observation_directory_identity(self.backend, &directory)?];
        let reads = ReadWindow::new(
            self.backend,
            access.configuration_root,
            &directory,
            scope,
            access.configuration,
            transport,
            checkpoint,
        )?;
        let result = self.verify_scope(&reads, scope);
        if result.is_err() {
            reads.write(
                "failure.json",
                &json!({"status":"post_copy_object_scope_failed","remote_writes":0}),
            )?;
        }
        result?;
        let plan_sha256 = digest_shape(&self.target["copy"]["plan_sha256"])?;
        for (index, ((bucket, key), item)) in self
            .objects
            .plan
            .iter()
            .filter(|(key, _)| !self.objects.covered.contains(*key))
            .enumerate()
        {
            let artifact = &item["artifact"];
            let directory = self.output.join(format!("remaining-object-{index:04}"));
            let observed = observe_object::observe(
                ObserveTarget {
                    backend: self.backend,
                    configuration_root: access.configuration_root,
                    work: self.output,
                    directory: &directory,
                    plan_sha256,
                    scope,
                    endpoint: access.endpoint,
                    region: access.region,
                    configuration: access.configuration,
                    bucket,
                    key,
                    expected: &json!({"bytes":artifact["bytes"],"sha256":artifact["sha256"]}),
                    max_bytes: artifact["bytes"].as_u64().ok_or("复制后补集对象大小无效")?,
                },
                transport,
                checkpoint,
            )?;
            if !matches!(observed.image_evidence(), ObjectObservation::Captured(_)) {
                return Err("补集对象在完整范围观察后缺失，保留原响应，不发布成功".into());
            }
            directories.push(observation_directory_identity(self.backend, &directory)?);
        }
        Ok(directories)
    }

    fn verify_scope<T: Transport>(&self, reads: &ReadWindow<'_, T>, scope: &str) -> Result<()> {
        let mut buckets = BUCKETS.to_vec();
        buckets.sort_unstable();
        let owners = reads.owners(&buckets, "owner")?;
        let owner_key = format!("{scope}/.ryframe-owner");
        let mut keys = BTreeMap::new();
        for bucket in buckets {
            let expected = self
                .objects
                .plan
                .keys()
                .filter(|(name, _)| name.as_str() == bucket)
                .map(|(_, key)| key.clone())
                .chain([owner_key.clone()])
                .collect::<BTreeSet<_>>();
            let current = reads.keys(bucket, "business")?;
            if current != expected {
                return Err("业务读取后目标完整五桶对象范围不同".into());
            }
            keys.insert(bucket, current);
        }
        reads.configuration()?;
        reads.write(
            "verified.json",
            &json!({"status":"post_copy_object_scope_verified","owners":owners,
            "keys":keys,"copy":self.target["copy"],"remote_writes":0}),
        )
    }
}

struct Objects<'a> {
    plan: BTreeMap<Key, &'a Value>,
    covered: BTreeSet<Key>,
    tenants: u64,
    posts: u64,
}

impl<'a> Objects<'a> {
    fn bind(copy_plan: &'a Value, dataset: &Value, scope: &str) -> Result<Self> {
        let mut plan = BTreeMap::new();
        for item in document::array(&copy_plan["objects"], "原复制计划对象")? {
            let bucket = document::text(&item["bucket"], "原复制计划对象桶")?;
            let key = document::text(&item["target_key"], "原复制计划目标对象 key")?;
            validate_key(scope, key, false)?;
            let artifact = &item["artifact"];
            if !BUCKETS.contains(&bucket)
                || artifact["bytes"].as_u64().is_none_or(|n| n == 0)
                || plan
                    .insert((bucket.to_owned(), key.to_owned()), item)
                    .is_some()
            {
                return Err("原复制对象桶、载荷大小或目标 key 无效".into());
            }
            digest_shape(&artifact["sha256"])?;
        }
        let tenants = document::array(&dataset["tenants"], "原业务租户样本")?;
        let mut covered = BTreeSet::new();
        let mut posts = 0_u64;
        for tenant in tenants {
            posts += document::array(&tenant["posts"], "原业务岗位样本")?.len() as u64;
            for file in document::array(&tenant["files"], "原业务对象样本")? {
                let path = document::text(&file["file_path"], "原业务对象路径")?;
                let key = ("uploads".to_owned(), format!("{scope}/{path}"));
                validate_key(scope, &key.1, false)?;
                let item = plan.get(&key).ok_or("原业务对象不属于复制计划 uploads")?;
                if file["bytes"] != item["artifact"]["bytes"]
                    || file["sha256"] != item["artifact"]["sha256"]
                    || !covered.insert(key)
                {
                    return Err("原业务对象数据集重复或载荷摘要与复制计划不同".into());
                }
            }
        }
        Ok(Self {
            plan,
            covered,
            tenants: tenants.len() as u64,
            posts,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/business_binding.rs"]
mod tests;

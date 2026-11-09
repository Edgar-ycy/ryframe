//! 在同一只读窗口复核全部已采集目标对象的当前 HEAD 和五桶范围。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    thread,
};

use serde_json::{Value, json};

use super::{
    super::{
        digest,
        export::transport::Transport,
        input::BUCKETS,
        json as document,
        resources::common,
        s3::{Operation, app_configuration, protocol::validate_key},
        target_verify::guard::Inputs,
    },
    images::{ObjectObservation, TargetObject, object_observed, object_pair},
    observe_object::Observed,
    observe_source::ReadWindow,
};
use crate::{
    Result,
    local_test_path::strip_local_test_prefix,
    recovery::{
        inputs::document as binding,
        source::evidence::{inspect_object, object_capture_result},
    },
};

const HEAD_WORKERS: usize = 4;

pub(crate) struct VerifyTarget<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) configuration_root: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) directory: &'a Path,
    pub(crate) plan_sha256: &'a str,
    pub(crate) generation_sha256: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) region: &'a str,
    pub(crate) configuration: &'a Value,
    pub(crate) items: &'a [Value],
    pub(crate) observations: &'a BTreeMap<(String, String), Observed>,
}

/// 调用方对共享的外层锁状态使用线程安全 checkpoint。
pub(crate) fn verify<T: Transport + Sync>(
    request: VerifyTarget<'_>,
    transport: &T,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
) -> Result<Value> {
    checkpoint()?;
    let selected = request.selected()?;
    common::local_directory(request.backend, request.work, "目标收尾工作根")?;
    if !below(request.directory, request.work) {
        return Err("目标收尾证据须使用本次工作根内的新目录".into());
    }
    common::ensure_new_directory(request.backend, request.directory, "目标对象收尾目录")?;
    let reads = ReadWindow::new(
        request.backend,
        request.configuration_root,
        request.directory,
        request.scope,
        request.configuration,
        transport,
        checkpoint,
    )?;
    let input = request.binding()?;
    let window = Window { request, reads };
    let mut stage = "capture_bindings";
    match window.run(&selected, &input, &mut stage) {
        Ok(receipt) => Ok(receipt),
        Err(_) => {
            let retained = window.reads.write("failure.json", &json!({
                "status":"target_heads_failed","stage":stage,"error_type":"TargetObjectSnapshotError",
                "binding":input,"business_downloads":0,"remote_writes":0,"restore_qualified":false,
            }));
            let detail = if retained.is_ok() {
                "保留原始证据"
            } else {
                "无法安全追加失败收据，保留现有文件"
            };
            Err(format!(
                "目标对象收尾未通过（{stage}），{detail}：{}",
                window.request.directory.display()
            )
            .into())
        }
    }
}

impl<'a> VerifyTarget<'a> {
    fn selected(&self) -> Result<BTreeMap<&'a str, BTreeSet<&'a str>>> {
        common::scope_id(self.scope)?;
        binding::digest(&json!(self.plan_sha256), "目标收尾计划摘要")?;
        binding::digest(&json!(self.generation_sha256), "目标收尾代次摘要")?;
        let mut selected = BUCKETS
            .iter()
            .map(|&bucket| (bucket, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for item in self.items {
            let bucket = document::text(&item["bucket"], "目标对象桶")?;
            let key = document::text(&item["target_key"], "目标对象 key")?;
            validate_key(self.scope, key, false)?;
            let keys = selected.get_mut(bucket).ok_or("计划对象桶不属于登记五桶")?;
            if !keys.insert(key)
                || !self
                    .observations
                    .contains_key(&(bucket.to_owned(), key.to_owned()))
            {
                return Err("目标 HEAD 复核必须覆盖最后完整核验的全部对象".into());
            }
        }
        if self.items.len() != self.observations.len() {
            return Err("目标完整采集集合包含计划外对象".into());
        }
        Ok(selected)
    }

    fn binding(&self) -> Result<Value> {
        Ok(json!({"plan_sha256":self.plan_sha256,
            "configuration_sha256":digest::plan_hash(&json!({"configuration":self.configuration}))?,
            "transport":"rust-s3-sigv4"}))
    }
}

struct Window<'a, T> {
    request: VerifyTarget<'a>,
    reads: ReadWindow<'a, T>,
}

impl<T: Transport + Sync> Window<'_, T> {
    fn run(
        &self,
        selected: &BTreeMap<&str, BTreeSet<&str>>,
        binding: &Value,
        stage: &mut &'static str,
    ) -> Result<Value> {
        let request = &self.request;
        if request.configuration["app_env"] != "test"
            || app_configuration::public_configuration(
                request.configuration_root,
                request.configuration,
            )? != json!({"endpoint":request.endpoint,"region":request.region})
        {
            return Err("目标收尾必须使用已绑定的 test 文件配置和原端点区域".into());
        }
        self.reads.configuration()?;
        self.reads.write("intent.json", &json!({
            "binding":binding,"generation_sha256":self.request.generation_sha256,"scope_id":self.request.scope,
            "endpoint":self.request.endpoint,"region":self.request.region,"selected":selected,
            "business_get_permitted":false,"remote_writes":0,
        }))?;
        let buckets = selected.keys().copied().collect::<Vec<_>>();
        *stage = "owners_before";
        let before = self.reads.owners(&buckets, "owner-before")?;
        *stage = "heads";
        let objects = self.heads(binding)?;
        *stage = "owners_after";
        let after = self.reads.owners(&buckets, "owner-after")?;
        *stage = "complete_listing";
        let keys = self.listings(selected)?;
        *stage = "owners_final";
        let final_owners = self.reads.owners(&buckets, "owner-final")?;
        *stage = "binding_after";
        self.reads.configuration()?;
        self.check_outputs(&objects, before.iter().chain(&after).chain(&final_owners))?;
        let receipt = json!({
            "status":"target_object_heads_verified","objects":objects,"binding":binding,
            "ownership_before":before,"ownership_after":after,"ownership_final":final_owners,
            "keys":keys,"business_downloads":0,"remote_writes":0,"restore_qualified":false,
        });
        *stage = "publish";
        self.reads.write("verified.json", &receipt)?;
        self.reads.configuration()?;
        if document::read(&self.request.directory.join("verified.json"))? != receipt {
            return Err("目标对象收尾收据在发布后变化".into());
        }
        Ok(receipt)
    }

    fn check_outputs<'a>(
        &self,
        heads: &[Value],
        owners: impl Iterator<Item = &'a Value>,
    ) -> Result<()> {
        let root = self.request.directory;
        for (index, row) in heads.iter().enumerate() {
            let path = digest::safe_file(root, &format!("head-{index}.json"))?;
            if row["head_sha256"] != digest::plan_hash(&document::read(&path)?)? {
                return Err("目标原始 HEAD 响应文件在收尾期间变化".into());
            }
        }
        for row in owners {
            let name = document::text(&row["file"], "目标原始 owner 正文")?;
            let actual = digest::file(&digest::safe_file(root, name)?)?;
            if row["bytes"].as_u64() != Some(actual.bytes) || row["sha256"] != actual.sha256 {
                return Err("目标原始 owner 正文在收尾期间变化".into());
            }
        }
        Ok(())
    }

    fn heads(&self, binding: &Value) -> Result<Vec<Value>> {
        let mut results = Vec::with_capacity(self.request.items.len());
        for (batch, items) in self.request.items.chunks(HEAD_WORKERS).enumerate() {
            let outcomes = thread::scope(|scope| {
                let handles = items
                    .iter()
                    .enumerate()
                    .map(|(offset, item)| {
                        scope.spawn(move || {
                            self.head(batch * HEAD_WORKERS + offset, item, binding)
                                .map_err(|_| "目标对象当前 HEAD 未通过")
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or(Err("目标对象 HEAD 工作线程未完成")))
                    .collect::<Vec<_>>()
            });
            for row in outcomes {
                results.push(row?);
            }
        }
        Ok(results)
    }

    fn head(&self, index: usize, item: &Value, binding: &Value) -> Result<Value> {
        let bucket = document::text(&item["bucket"], "目标对象桶")?;
        let key = document::text(&item["target_key"], "目标对象 key")?;
        let observed = &self.request.observations[&(bucket.to_owned(), key.to_owned())];
        let proof = Capture::open(&self.request, item, observed, binding)?;
        let identity = &proof.value["identity"];
        let etag = document::text(&identity["ETag"], "原目标采集 ETag")?;
        let version = match &identity["VersionId"] {
            Value::Null => None,
            value => Some(document::text(value, "原目标采集版本")?.to_owned()),
        };
        let raw = self.reads.call(
            &format!("head-{index}"),
            "head-object",
            bucket,
            Operation::Head {
                key: key.to_owned(),
                if_match: Some(etag.to_owned()),
                version,
            },
        )?;
        let inspected = inspect_object(&raw)?;
        let sha = digest::plan_hash(&raw)?;
        if inspected["identity"] != *identity
            || inspected["metadata"] != proof.value["metadata"]
            || sha
                != binding::digest(
                    &proof.value["response_sha256"]["head_before"],
                    "原目标 HEAD 完整响应",
                )?
        {
            return Err("目标对象在完整采集后被替换，或 HEAD 元数据和原响应发生变化".into());
        }
        proof.check()?;
        Ok(json!({"resource":observed.receipt()["resource"],"head_sha256":sha}))
    }

    fn listings(
        &self,
        selected: &BTreeMap<&str, BTreeSet<&str>>,
    ) -> Result<BTreeMap<String, BTreeSet<String>>> {
        let owner = format!("{}/.ryframe-owner", self.request.scope);
        let mut listed = BTreeMap::new();
        let mut matched = true;
        for (&bucket, keys) in selected {
            let expected = keys
                .iter()
                .copied()
                .chain([owner.as_str()])
                .collect::<BTreeSet<_>>();
            let actual = self.reads.keys(bucket, "final")?;
            matched &= actual.iter().map(String::as_str).collect::<BTreeSet<_>>() == expected;
            listed.insert(bucket.to_owned(), actual);
        }
        if !matched {
            return Err("目标五桶完整对象范围有遗漏、额外对象或 owner 缺失".into());
        }
        Ok(listed)
    }
}

struct Capture<'a> {
    value: &'a Value,
    directory: PathBuf,
    evidence: BTreeSet<String>,
    protected: Inputs,
}

impl<'a> Capture<'a> {
    fn open(
        request: &VerifyTarget<'_>,
        item: &Value,
        observed: &'a Observed,
        input: &Value,
    ) -> Result<Self> {
        let ObjectObservation::Captured(value) = observed.image_evidence() else {
            return Err("目标收尾缺少实际完整字节采集".into());
        };
        let resource = || TargetObject {
            scope: request.scope,
            endpoint: request.endpoint,
            item,
        };
        let (_, expected_after) = object_pair(request.generation_sha256, resource())?;
        if object_observed(
            request.generation_sha256,
            resource(),
            ObjectObservation::Captured(value),
        )? != expected_after
            || value["max_bytes"] != item["artifact"]["bytes"]
            || value["region"] != request.region
            || value["get_header_differences"] != json!([])
        {
            return Err("目标完整采集与计划后像、字节上限或存储元数据不同".into());
        }
        let receipt = observed.receipt();
        let path =
            common::bound_file(request.backend, observed.descriptor(), "原目标对象观察收据")?;
        let directory = path.parent().ok_or("原目标观察收据缺少目录")?;
        if !below(directory, request.work)
            || path.file_name().and_then(|name| name.to_str()) != Some("observation.json")
            || receipt["status"] != "object_present"
            || receipt["binding"] != *input
            || receipt["resource"]
                != json!({"kind":"object","scope_id":request.scope,
                "endpoint":request.endpoint,"bucket":item["bucket"],"key":item["target_key"]})
            || receipt["remote_writes"].as_u64() != Some(0)
        {
            return Err("完整采集收据不属于同一工作根、目标或计划配置".into());
        }
        let capture =
            common::bound_file(request.backend, &receipt["capture"], "原目标完整采集收据")?;
        if capture != directory.join("capture.json") {
            return Err("目标观察收据未绑定同目录原采集".into());
        }
        let (evidence, protected) = protect_capture(directory, &path, &receipt["evidence"])?;
        if common::read_bound(request.backend, observed.descriptor(), "原目标对象观察收据")?
            != *receipt
            || common::read_bound(request.backend, &receipt["capture"], "原目标完整采集收据")?
                != *value
            || !evidence.contains("capture.json")
        {
            return Err("磁盘目标观察或完整采集收据与原绑定不同".into());
        }
        let proof = Self {
            value,
            directory: directory.to_path_buf(),
            evidence,
            protected,
        };
        proof.recompute(receipt)?;
        proof.check()?;
        Ok(proof)
    }

    fn recompute(&self, receipt: &Value) -> Result<()> {
        let read = |name: &str| {
            if !self.evidence.contains(name) {
                return Err("目标采集缺少明确原始证据".into());
            }
            document::read(&self.directory.join(name))
        };
        let mut intent = read("intent.json")?;
        let intent_fields = intent.as_object_mut().ok_or("原目标采集意图无效")?;
        if intent_fields.remove("binding").as_ref() != Some(&receipt["binding"]) {
            return Err("原目标采集意图和观察收据绑定不同".into());
        }
        intent["kind"] = json!("devex-clone-object-capture");
        if !self.evidence.contains("object.bin") {
            return Err("目标采集缺少已核完整载荷".into());
        }
        let actual = &receipt["evidence"]["object.bin"];
        let calculated = object_capture_result(
            &intent,
            &read("head-before.json")?,
            &read("get.json")?,
            &read("head-after.json")?,
            actual,
            document::array(&receipt["ownership_before"], "原目标采集前 owner")?.to_vec(),
            document::array(&receipt["ownership_after"], "原目标采集后 owner")?.to_vec(),
        )?;
        if &calculated != self.value {
            return Err("原目标采集与保留的 HEAD、GET、载荷和 owner 不同".into());
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        let present = fs::read_dir(&self.directory)?
            .map(|entry| {
                entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| "原目标采集证据文件名无效".into())
            })
            .collect::<Result<BTreeSet<_>>>()?;
        if present != self.evidence {
            return Err("原目标采集证据集合缺失或出现额外文件".into());
        }
        self.protected.check()
    }
}

fn protect_capture(
    directory: &Path,
    observation: &Path,
    files: &Value,
) -> Result<(BTreeSet<String>, Inputs)> {
    let files = document::object(files, "原目标观察证据")?;
    let mut evidence = BTreeSet::from(["observation.json".to_owned()]);
    let mut paths = BTreeMap::new();
    for (name, descriptor) in files {
        document::exact(descriptor, &["bytes", "sha256"], "目标原始证据文件")?;
        if name.contains('/') || !evidence.insert(name.clone()) {
            return Err("目标原始证据不是独立平面文件集合".into());
        }
        let file = digest::safe_file(directory, name)?;
        if descriptor["bytes"].as_u64() != Some(fs::metadata(&file)?.len()) {
            return Err("目标原始采集证据字节数变化".into());
        }
        paths.insert(file, descriptor);
    }
    let protected = Inputs::open(paths.keys().cloned().chain([observation.to_path_buf()]))?;
    for (path, descriptor) in paths {
        let actual = digest::file(&path)?;
        if descriptor["bytes"].as_u64() != Some(actual.bytes)
            || descriptor["sha256"] != actual.sha256
        {
            return Err("目标原始采集证据摘要变化".into());
        }
    }
    Ok((evidence, protected))
}

fn below(path: &Path, root: &Path) -> bool {
    strip_local_test_prefix(path, root).is_some_and(|relative| !relative.as_os_str().is_empty())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/verify_target_objects.rs"]
mod tests;

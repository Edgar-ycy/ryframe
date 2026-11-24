//! 从同一完整恢复资源像重验四库库存、对象采集及最终 HEAD 证据。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Value, json};

use super::{bound, require_absent};
use crate::{
    Result,
    recovery::{
        clone::native::{
            digest, s3::protocol::validate_key, target_verify::evidence_protocol::inventory_history,
        },
        inputs::{
            document::{self, Document},
            domain::BUCKETS,
        },
        source::evidence::object_capture_result,
    },
};

pub(super) fn read_image(
    backend: &Path,
    work: &Path,
    phase: &str,
    descriptor: &Value,
    target: &Document,
) -> Result<Document> {
    let directory = work.join(format!("restore-candidate-{phase}"));
    require_absent(&directory.join("failure.json"))?;
    let file = fixed(
        backend,
        &directory.join("image.json"),
        descriptor,
        "原恢复整体像",
    )?;
    let value = &file.value;
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "phase",
            "target_plan",
            "inventory",
            "object_observations",
            "image",
            "expected_sha256",
            "matches_expected",
        ],
        "原恢复整体像",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "restore-reference-resource-image"
        || value["phase"] != phase
        || value["target_plan"] != document::binding(target)
        || value["matches_expected"] != true
    {
        return Err("原恢复整体像未成功或未绑定同一目标和阶段".into());
    }
    Ok(file)
}

pub(super) struct ObjectPlan<'a> {
    pub(super) manifest: &'a Value,
    pub(super) reference: &'a Value,
    pub(super) generation: &'a Value,
}

pub(super) fn check_image(
    backend: &Path,
    file: &Document,
    expected: &Value,
    initialized: &Path,
    target_request: &Value,
    objects_from: Option<ObjectPlan<'_>>,
) -> Result<()> {
    let value = &file.value;
    if value["image"] != *expected || value["expected_sha256"] != digest::plan_hash(expected)? {
        return Err("原恢复整体像的内容与同一正式备份和初始化记录不同".into());
    }
    let phase = file.path.parent().ok_or("原恢复整体像缺少阶段目录")?;
    let directory = phase.join("inventory");
    require_absent(&directory.join("failure.json"))?;
    require_absent(&directory.join("initialize.lock"))?;
    let inventory = fixed(
        backend,
        &directory.join("inventory.json"),
        &value["inventory"],
        "原恢复库存",
    )?;
    if inventory.value["status"] != "side_inventory_captured"
        || inventory.value["side"] != "target"
        || inventory.value["remote_writes"] != 0
    {
        return Err("原恢复整体像未绑定已采集的目标库存".into());
    }
    inventory_history::verify(
        backend,
        initialized,
        &json!({
            "initial":{"receipt":value["inventory"],"observations":expected["databases"]},
            "request":target_request, "resumed":false, "inventory_directory":directory,
        }),
    )?;
    let observations = document::array(&value["object_observations"], "原对象观察集合")?;
    let Some(plan) = objects_from else {
        if !observations.is_empty() {
            return Err("原恢复前像含非前像对象观察".into());
        }
        return Ok(());
    };
    Objects::new(backend, plan, &expected["objects"])?.verify(phase, observations)
}

struct Objects<'a> {
    backend: &'a Path,
    plan: ObjectPlan<'a>,
    expected: &'a Value,
    prefix: String,
    scope: &'a str,
    endpoint: &'a str,
    region: &'a str,
    binding: Value,
}

impl<'a> Objects<'a> {
    fn new(backend: &'a Path, plan: ObjectPlan<'a>, expected: &'a Value) -> Result<Self> {
        let target = &plan.reference["target"];
        let scope = document::text(&target["scope_id"], "原目标对象 scope")?;
        let s3 = &target["s3"];
        let binding = json!({
            "plan_sha256":digest::plan_hash(plan.reference)?,
            "configuration_sha256":digest::plan_hash(&json!({"configuration":s3["configuration"]}))?,
            "transport":"rust-s3-sigv4",
        });
        let source = document::text(&plan.manifest["scope_id"], "原备份 scope")?;
        let prefix = format!("{source}/");
        Ok(Self {
            backend,
            plan,
            expected,
            scope,
            binding,
            prefix,
            endpoint: document::text(&s3["endpoint"], "原目标对象端点")?,
            region: document::text(&s3["region"], "原目标对象区域")?,
        })
    }

    fn verify(&self, phase: &Path, descriptors: &[Value]) -> Result<()> {
        let mut descriptors = descriptors.iter();
        let mut heads = Vec::new();
        let mut selected = BTreeMap::<&str, BTreeSet<String>>::new();
        for row in document::array(&self.plan.manifest["objects"], "原备份对象桶")? {
            let bucket = document::text(&row["bucket"], "原备份对象桶")?;
            if !BUCKETS.contains(&bucket) || selected.contains_key(bucket) {
                return Err("原恢复后像对象桶未知或重复".into());
            }
            let mut keys = BTreeSet::new();
            for (index, item) in document::array(&row["entries"], "原备份对象")?
                .iter()
                .enumerate()
            {
                let original = document::text(&item["key"], "原备份对象键")?;
                let suffix = original
                    .strip_prefix(&self.prefix)
                    .ok_or("原备份对象不属于同一 scope")?;
                let key = format!("{}/{suffix}", self.scope);
                validate_key(self.scope, &key, false)?;
                if !keys.insert(key.clone()) {
                    return Err("原恢复后像对象键重复".into());
                }
                let directory = phase.join(format!("object-{bucket}-{index}"));
                let descriptor = descriptors.next().ok_or("原恢复后像缺少对象观察")?;
                heads.push(self.capture(&directory, descriptor, bucket, &key, item)?);
            }
            selected.insert(bucket, keys);
        }
        let head = descriptors.next().ok_or("原恢复后像缺少最终对象证明")?;
        if selected.len() != BUCKETS.len() || descriptors.next().is_some() {
            return Err("原恢复后像的最终对象证明或观察范围不同".into());
        }
        self.verify_heads(&phase.join("object-heads"), head, &heads, &selected)
    }

    fn capture(
        &self,
        directory: &Path,
        descriptor: &Value,
        bucket: &str,
        key: &str,
        item: &Value,
    ) -> Result<Value> {
        require_absent(&directory.join("failure.json"))?;
        let observation = fixed(
            self.backend,
            &directory.join("observation.json"),
            descriptor,
            "原对象观察",
        )?;
        let receipt = &observation.value;
        let resource = json!({"kind":"object","scope_id":self.scope,"endpoint":self.endpoint,"bucket":bucket,"key":key});
        check_object_receipt(receipt, &resource, &self.binding)?;
        let expected = json!({"bytes":item["bytes"],"sha256":item["sha256"]});
        let intent = json!({"format_version":1,"kind":"devex-clone-object-capture","side":"target",
            "scope_id":self.scope,"endpoint":self.endpoint,"region":self.region,"bucket":bucket,
            "key":key,"expected":expected,"max_bytes":item["bytes"],"transport":"rust-s3-sigv4","clone_verified":false});
        let files = &receipt["evidence"];
        let saved_intent = raw_json(self.backend, directory, "intent.json", files)?;
        let mut observed_intent = intent.clone();
        observed_intent["kind"] = json!("devex-clone-live-object");
        observed_intent["binding"] = self.binding.clone();
        if saved_intent.value != observed_intent {
            return Err("原对象采集意图与正式目标、载荷或配置绑定不同".into());
        }
        let body = raw_digest(directory, "object.bin", &files["object.bin"])?;
        let head = raw_json(self.backend, directory, "head-before.json", files)?;
        let get = raw_json(self.backend, directory, "get.json", files)?;
        let repeated = raw_json(self.backend, directory, "head-after.json", files)?;
        let before = vec![self.owner(directory, "owner-before", bucket)?];
        let after = vec![self.owner(directory, "owner-after", bucket)?];
        for (name, rows) in [("ownership_before", &before), ("ownership_after", &after)] {
            let row = &rows[0];
            let file = document::text(&row["file"], "原对象 owner 正文路径")?;
            if receipt[name] != json!(rows)
                || files[file] != json!({"bytes":row["bytes"],"sha256":row["sha256"]})
            {
                return Err("原对象采集前后 owner 与已登记 scope 不同".into());
            }
        }
        let result = object_capture_result(
            &intent,
            &head.value,
            &get.value,
            &repeated.value,
            &body,
            before,
            after,
        )?;
        let capture = raw_json(self.backend, directory, "capture.json", files)?;
        if receipt["capture"] != document::binding(&capture)
            || capture.value != result
            || result["get_header_differences"] != json!([])
        {
            return Err("原对象采集与实际 HEAD、GET、载荷、元数据或绑定不同".into());
        }
        Ok(json!({"resource":resource,"head_sha256":result["response_sha256"]["head_before"]}))
    }

    fn verify_heads(
        &self,
        directory: &Path,
        descriptor: &Value,
        expected: &[Value],
        selected: &BTreeMap<&str, BTreeSet<String>>,
    ) -> Result<()> {
        require_absent(&directory.join("failure.json"))?;
        let heads = fixed(
            self.backend,
            &directory.join("verified.json"),
            descriptor,
            "原最终对象证明",
        )?;
        let value = &heads.value;
        document::exact(
            value,
            &[
                "status",
                "objects",
                "binding",
                "ownership_before",
                "ownership_after",
                "ownership_final",
                "keys",
                "business_downloads",
                "remote_writes",
                "restore_qualified",
            ],
            "原最终对象证明",
        )?;
        let keys = self
            .expected
            .as_object()
            .ok_or("原五桶后像缺少键范围")?
            .iter()
            .map(|(bucket, value)| (bucket, &value["keys"]))
            .collect::<BTreeMap<_, _>>();
        if value["status"] != "target_object_heads_verified"
            || value["binding"] != self.binding
            || value["objects"] != json!(expected)
            || value["keys"] != json!(keys)
            || value["business_downloads"] != 0
            || value["remote_writes"] != 0
            || value["restore_qualified"] != false
        {
            return Err("原最终对象证明未覆盖同一采集、完整五桶或计划绑定".into());
        }
        let intent = document::read(
            &directory.join("intent.json"),
            self.backend,
            "原最终对象意图",
        )?;
        if intent.value
            != json!({"binding":self.binding,"generation_sha256":digest::plan_hash(self.plan.generation)?,
            "scope_id":self.scope,"endpoint":self.endpoint,"region":self.region,"selected":selected,
            "business_get_permitted":false,"remote_writes":0})
        {
            return Err("原最终对象意图与采集代次或范围不同".into());
        }
        for (index, row) in expected.iter().enumerate() {
            let file = directory.join(format!("head-{index}.json"));
            let raw = document::read(&file, self.backend, "原最终对象 HEAD")?;
            if row["head_sha256"] != digest::plan_hash(&raw.value)? {
                return Err("原最终对象 HEAD 与同一对象采集的完整响应不同".into());
            }
        }
        for (name, phase) in [
            ("ownership_before", "owner-before"),
            ("ownership_after", "owner-after"),
            ("ownership_final", "owner-final"),
        ] {
            let rows = selected
                .keys()
                .map(|bucket| self.owner(directory, phase, bucket))
                .collect::<Result<Vec<_>>>()?;
            if value[name] != json!(rows) {
                return Err("原最终对象证明的五桶 owner 不同".into());
            }
        }
        Ok(())
    }

    fn owner(&self, directory: &Path, phase: &str, bucket: &str) -> Result<Value> {
        let name = format!("{phase}-{bucket}.bin");
        let owner = raw_digest(directory, &name, &self.expected[bucket]["owner"])?;
        Ok(
            json!({"bucket":bucket,"key":format!("{}/.ryframe-owner",self.scope),"file":name,
            "bytes":owner["bytes"],"sha256":owner["sha256"]}),
        )
    }
}

fn check_object_receipt(receipt: &Value, resource: &Value, binding: &Value) -> Result<()> {
    document::exact(
        receipt,
        &[
            "format_version",
            "status",
            "resource",
            "binding",
            "evidence",
            "ownership_before",
            "ownership_after",
            "capture",
            "remote_writes",
            "clone_verified",
            "restore_qualified",
        ],
        "原对象观察收据",
    )?;
    if receipt["format_version"] != 1
        || receipt["status"] != "object_present"
        || receipt["resource"] != *resource
        || receipt["binding"] != *binding
        || receipt["remote_writes"] != 0
        || receipt["clone_verified"] != false
        || receipt["restore_qualified"] != false
    {
        return Err("原对象观察未绑定同一对象、目标 scope 或计划配置".into());
    }
    Ok(())
}

fn raw_json(backend: &Path, directory: &Path, name: &str, files: &Value) -> Result<Document> {
    let binding = document::exact(&files[name], &["bytes", "sha256"], "原对象证据绑定")?;
    let path = directory.join(name);
    fixed(
        backend,
        &path,
        &json!({"path":path,"bytes":binding["bytes"],"sha256":binding["sha256"]}),
        "原对象原始证据",
    )
}

fn raw_digest(directory: &Path, name: &str, descriptor: &Value) -> Result<Value> {
    document::exact(descriptor, &["bytes", "sha256"], "原对象正文绑定")?;
    let file = digest::safe_file(directory, name)?;
    let actual = digest::file(&file)?;
    let value = json!({"bytes":actual.bytes,"sha256":actual.sha256});
    if value != *descriptor {
        return Err("原对象或 owner 正文的实际字节与摘要不同".into());
    }
    Ok(value)
}

fn fixed(backend: &Path, expected: &Path, descriptor: &Value, label: &str) -> Result<Document> {
    if document::binding_shape(descriptor, label)?.path != expected {
        return Err("独立 B1 已完成恢复证明路径与固定阶段不同".into());
    }
    bound(backend, descriptor, label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::runtime::dataset_authority::reads::Window;

    #[test]
    fn completed_object_observation_rejects_same_bucket_wrong_key() -> Result<()> {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        std::fs::create_dir_all(&local)?;
        let temporary = tempfile::Builder::new()
            .prefix("completed-b1-object-")
            .tempdir_in(local)?;
        let directory = temporary.path();
        let manifest = json!({"scope_id":"source"});
        let reference = json!({"target":{"scope_id":"target","s3":{
            "endpoint":"http://127.0.0.1:19000","region":"test-region","configuration":{}
        }}});
        let generation = json!({});
        let expected = json!({});
        let objects = Objects::new(
            &backend,
            ObjectPlan {
                manifest: &manifest,
                reference: &reference,
                generation: &generation,
            },
            &expected,
        )?;
        let resource = json!({"kind":"object","scope_id":"target","endpoint":"http://127.0.0.1:19000",
            "bucket":"uploads","key":"target/another-object"});
        let receipt = json!({"format_version":1,"status":"object_present","resource":resource,
            "binding":objects.binding,"evidence":{},"ownership_before":[],"ownership_after":[],
            "capture":null,"remote_writes":0,"clone_verified":false,"restore_qualified":false});
        let path = directory.join("observation.json");
        std::fs::write(&path, document::canonical_bytes(&receipt)?)?;
        let window = Window::direct()?;
        let saved = document::read(&path, &backend, "测试原对象观察")?;
        let item = json!({"bytes":9,"sha256":"a".repeat(64)});
        let error = objects
            .capture(
                directory,
                &document::binding(&saved),
                "uploads",
                "target/expected-object",
                &item,
            )
            .unwrap_err();
        assert!(error.to_string().contains("同一对象"));
        window.finish(Ok(()))
    }
}

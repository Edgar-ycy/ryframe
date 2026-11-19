use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Value, json};

use super::{
    super::{
        super::{
            digest,
            export::{bound_relative, transport::Transport},
            input::BUCKETS,
            json as document,
            resources::common,
            s3::protocol::validate_key,
            target_verify::{guard::Inputs, tree},
        },
        images::ObjectObservation,
    },
    Key, ObjectInputs, ObjectPort, Observed, Sample, Step, item_key, reject_failure,
};
use crate::{
    Result, local_test_path::strip_local_test_prefix, recovery::inputs::document as proof,
};

impl<T: Transport + Sync> ObjectPort<'_, T> {
    pub(super) fn check_export(&self) -> Result<()> {
        common::bound_file(self.spec.backend, self.spec.export, "原对象导出")?;
        reject_failure(self.spec.export_root)
    }

    pub(super) fn check_sources(&self, steps: &[&Step<'_>]) -> Result<()> {
        self.check_export()?;
        for step in steps {
            self.check_source(step.item)?;
        }
        Ok(())
    }

    pub(super) fn check_all_source(&self) -> Result<()> {
        self.check_export()?;
        for ((bucket, key), original) in &self.source_entries {
            self.check_saved(bucket, key, original)?;
        }
        self.check_export()
    }

    pub(super) fn check_source(&self, item: &Value) -> Result<()> {
        let key = item_key(item, "source_key")?;
        let original = self
            .source_entries
            .get(&key)
            .ok_or("原导出缺少本步骤对象")?;
        if original["artifact"] != item["artifact"] || original["metadata"] != item["metadata"] {
            return Err("对象步骤载荷或 metadata 不属于原导出对象".into());
        }
        self.check_saved(&key.0, &key.1, original)
    }

    fn check_saved(&self, bucket: &str, key: &str, original: &Value) -> Result<()> {
        let descriptor = &original["capture"];
        let relative = document::text(&descriptor["file"], "原对象采集路径")?;
        let capture = bound_relative(self.spec.export_root, descriptor, relative)?;
        let directory = capture.parent().ok_or("原对象采集缺少目录")?;
        reject_failure(directory)?;
        let files = self
            .proof_files
            .get(directory)
            .ok_or("原对象缺少已核小型证据")?;
        if !files.iter().any(|(path, _)| *path == capture.as_path()) {
            return Err("原对象小型证据未绑定采集收据".into());
        }
        let held = Inputs::open(files.iter().map(|(path, _)| path.to_path_buf()))?;
        for (path, descriptor) in files {
            let actual = digest::file(path)?;
            if descriptor["bytes"].as_u64() != Some(actual.bytes)
                || descriptor["sha256"] != actual.sha256
            {
                return Err("本对象原始小型证据发生变化".into());
            }
        }
        if document::read(&capture)? != self.spec.captures[bucket][key] {
            return Err("原对象采集值与冻结证明不同".into());
        }
        held.check()?;
        reject_failure(directory)
    }

    pub(super) fn check_source_sample(&self, sample: &Sample) -> Result<()> {
        let (binding, receipt) = proof::bound_file(
            self.spec.backend,
            &sample.source_descriptor,
            "本批源对象观察",
        )?;
        let directory = sample_directory(&binding.path, self.spec.work)?;
        let (bucket, key) = item_key(&sample.item, "source_key")?;
        let observed = &receipt.value["observed"];
        let head = document::read(&directory.join("head-0.json"))?;
        let head_sha256 = digest::plan_hash(&head)?;
        let frozen = &self.spec.captures[&bucket][&key];
        if receipt.value != sample.source_receipt
            || receipt.value["export_binding"] != *self.spec.export
            || receipt.value["whole_source"] != false
            || frozen["response_sha256"]["head_before"] != head_sha256
            || observed
                != &json!([{"bucket":bucket,"key":key,
                "head_sha256":head_sha256,"identity":frozen["identity"]}])
            || receipt.value["identity_sha256"] != digest::plan_hash(observed)?
            || tree::snapshot(self.spec.backend, directory, false)? != sample.source_files
        {
            return Err("本批源对象观察、原 HEAD 或证明集合变化".into());
        }
        reject_failure(directory)
    }

    pub(super) fn check_target_sample(&self, observed: &Observed) -> Result<()> {
        let (binding, receipt) =
            proof::bound_file(self.spec.backend, observed.descriptor(), "本批目标对象观察")?;
        let directory = sample_directory(&binding.path, self.spec.work)?;
        if receipt.value != *observed.receipt() {
            return Err("本批目标对象观察收据变化".into());
        }
        let mut expected =
            document::object(&receipt.value["evidence"], "本批目标原始证据")?.clone();
        if expected
            .insert(
                "observation.json".into(),
                json!({"bytes":binding.bytes,"sha256":binding.sha256}),
            )
            .is_some()
        {
            return Err("本批目标原始证据自引用观察收据".into());
        }
        let actual = tree::snapshot(self.spec.backend, directory, false)?;
        let actual = document::array(&actual, "本批目标证据树")?;
        if actual.len() != expected.len()
            || actual.iter().any(|row| {
                row["type"] != "file"
                    || row["path"]
                        .as_str()
                        .and_then(|path| expected.get(path))
                        .is_none_or(|file| {
                            file["bytes"] != row["bytes"] || file["sha256"] != row["sha256"]
                        })
            })
        {
            return Err("本批目标对象原始证据缺失、改变或包含额外文件".into());
        }
        if let ObjectObservation::Captured(value) = observed.image_evidence() {
            let (binding, capture) = proof::bound_file(
                self.spec.backend,
                &receipt.value["capture"],
                "本批目标完整采集",
            )?;
            if binding.path != directory.join("capture.json") || capture.value != *value {
                return Err("本批目标完整采集与原证据不同".into());
            }
        }
        reject_failure(directory)
    }
}

pub(super) fn source_index(
    spec: &ObjectInputs<'_>,
    objects: &Value,
) -> Result<BTreeMap<Key, Value>> {
    let mut index = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for row in document::array(objects, "原导出五桶")? {
        let bucket = document::text(&row["bucket"], "原导出桶名")?;
        let items = document::array(&row["entries"], "原导出对象")?;
        let captures = spec.captures.get(bucket).ok_or("冻结证明缺少原导出桶")?;
        if !BUCKETS.contains(&bucket) || !seen.insert(bucket) || captures.len() != items.len() {
            return Err("原导出五桶及冻结采集不能一一对应".into());
        }
        for item in items {
            let key = document::text(&item["key"], "原导出对象 key")?;
            validate_key(spec.source.scope, key, false)?;
            if !captures.contains_key(key)
                || index
                    .insert((bucket.into(), key.into()), item.clone())
                    .is_some()
            {
                return Err("原导出对象缺少冻结采集或重复".into());
            }
        }
    }
    if seen.len() != BUCKETS.len() || spec.captures.len() != seen.len() {
        return Err("原导出和冻结证明必须完整覆盖五桶".into());
    }
    Ok(index)
}

fn sample_directory<'a>(path: &'a Path, work: &Path) -> Result<&'a Path> {
    let parent = path.parent().ok_or("对象观察收据缺少目录")?;
    if path.file_name().and_then(|name| name.to_str()) != Some("observation.json")
        || strip_local_test_prefix(parent, work)
            .is_none_or(|relative| relative.as_os_str().is_empty())
    {
        return Err("对象观察收据不在本次复制工作根".into());
    }
    reject_failure(parent)?;
    Ok(parent)
}

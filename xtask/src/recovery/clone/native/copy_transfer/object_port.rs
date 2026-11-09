//! 将同一复制会话的源证明、目标观察和条件创建接到对象步骤。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    thread,
};

use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    super::{
        copy_ledger::session::Session,
        digest,
        export::transport::Transport,
        input::{self, BUCKETS, digest_shape},
        json as document,
        resources::common,
        s3::{app_configuration, protocol::validate_key},
        target_verify::tree,
    },
    images::{self, TargetObject},
    object_write,
    observe_object::{self, ObserveTarget, Observed},
    observe_source::{self, ObserveSource, ReadWindow, Selection},
    steps::{Basis, Kind, Observation, Step},
    verify_target_objects::{self, VerifyTarget},
};
use crate::{
    Result,
    recovery::{inputs::document as proof, source::evidence::reference::copy_step_ids},
};

mod proofs;
use proofs::{reject_failure, source_index};

const WORKERS: usize = 4;
type Key = (String, String);
type ProofFiles<'a> = BTreeMap<PathBuf, Vec<(&'a Path, &'a Value)>>;

pub(in crate::recovery::clone::native) struct Side<'a> {
    pub(crate) scope: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) region: &'a str,
    pub(crate) configuration: &'a Value,
}

pub(in crate::recovery::clone::native) struct ObjectInputs<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) target_configuration_root: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) export_root: &'a Path,
    pub(crate) source: Side<'a>,
    pub(crate) target: Side<'a>,
    pub(crate) export: &'a Value,
    pub(crate) captures: &'a BTreeMap<String, BTreeMap<String, Value>>,
    pub(crate) proof_files: &'a Value,
    pub(crate) plan_sha256: &'a str,
    pub(crate) generation_sha256: &'a str,
}

pub(in crate::recovery::clone::native) struct ObjectPort<'a, T> {
    spec: ObjectInputs<'a>,
    transport: &'a T,
    source_entries: BTreeMap<Key, Value>,
    proof_files: ProofFiles<'a>,
    latest: BTreeMap<Key, Observed>,
}

pub(in crate::recovery::clone::native) struct Evidence(Vec<Sample>);

struct Sample {
    step: String,
    item: Value,
    source_receipt: Value,
    source_descriptor: Value,
    source_files: Value,
    target_receipt: Value,
    target_descriptor: Value,
}

impl<'a, T: Transport + Sync> ObjectPort<'a, T> {
    pub(in crate::recovery::clone::native) fn new(
        spec: ObjectInputs<'a>,
        transport: &'a T,
    ) -> Result<Self> {
        digest_shape(&json!(spec.plan_sha256))?;
        digest_shape(&json!(spec.generation_sha256))?;
        common::scope_id(spec.source.scope)?;
        common::scope_id(spec.target.scope)?;
        common::local_directory(spec.backend, spec.work, "对象复制工作根")?;
        common::local_directory(spec.backend, spec.export_root, "原对象导出目录")?;
        reject_failure(spec.export_root)?;
        let (binding, export) = proof::bound_file(spec.backend, spec.export, "原对象导出")?;
        if binding.path != spec.export_root.join("export.json")
            || spec.source.scope == spec.target.scope
            || export.value["scope_id"] != spec.source.scope
            || export.value["source"]["s3"]["endpoint"] != spec.source.endpoint
            || export.value["source"]["s3"]["region"] != spec.source.region
        {
            return Err("原对象导出、两侧 scope 或原端点绑定不同".into());
        }
        let source_entries = source_index(&spec, &export.value["objects"])?;
        let mut proof_files = ProofFiles::new();
        for (path, expected) in document::object(spec.proof_files, "原导出小型证明集合")? {
            let path = Path::new(path);
            let parent = path.parent().ok_or("原导出小型证明缺少父目录")?;
            proof_files
                .entry(parent.to_path_buf())
                .or_default()
                .push((path, expected));
        }
        Ok(Self {
            spec,
            transport,
            source_entries,
            proof_files,
            latest: BTreeMap::new(),
        })
    }

    pub(in crate::recovery::clone::native) fn basis(&self, step: &Step<'_>) -> Result<Basis> {
        self.check_step(step)?;
        self.check_export()?;
        self.check_source(step.item)?;
        let artifact = input::bound_file(self.spec.export_root, &step.item["artifact"])?;
        if artifact.digest.sha256 != step.artifact {
            return Err("复制对象原载荷摘要与本步骤不同".into());
        }
        self.pair(step.item)
    }

    pub(in crate::recovery::clone::native) fn observe(
        &mut self,
        selected: &[&Step<'_>],
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Observation<Evidence>> {
        if selected.is_empty() || selected.len() > WORKERS {
            return Err("对象观察批次须包含一至四个已登记步骤".into());
        }
        let mut unique = BTreeSet::new();
        for step in selected {
            self.check_step(step)?;
            if !unique.insert(&step.id) {
                return Err("对象观察批次步骤重复".into());
            }
        }
        checkpoint()?;
        self.check_sources(selected)?;
        let sources = batch(selected, |step| self.sample_source(step, checkpoint))?;
        self.check_sources(selected)?;
        let targets = batch(selected, |step| self.sample_target(step, checkpoint))?;
        checkpoint()?;
        self.check_sources(selected)?;
        let mut images = Map::new();
        let mut rows = Vec::with_capacity(selected.len());
        let mut latest = Vec::with_capacity(selected.len());
        for ((step, (source_receipt, source_descriptor, source_files)), observed) in
            selected.iter().zip(sources).zip(targets)
        {
            images.insert(step.id.clone(), self.image(step.item, &observed)?);
            rows.push(Sample {
                step: step.id.clone(),
                item: step.item.clone(),
                source_receipt,
                source_descriptor,
                source_files,
                target_receipt: observed.receipt().clone(),
                target_descriptor: observed.descriptor().clone(),
            });
            latest.push((item_key(step.item, "target_key")?, observed));
        }
        self.latest.extend(latest);
        Ok(Observation {
            images,
            evidence: Evidence(rows),
        })
    }

    pub(in crate::recovery::clone::native) fn check_observation(
        &self,
        images: &Map<String, Value>,
        evidence: &Evidence,
    ) -> Result<()> {
        if images.len() != evidence.0.len() {
            return Err("对象图片向量与本批原证据数量不同".into());
        }
        self.check_export()?;
        for sample in &evidence.0 {
            self.check_source(&sample.item)?;
            self.check_source_sample(sample)?;
            let observed = self
                .latest
                .get(&item_key(&sample.item, "target_key")?)
                .ok_or("对象观察已脱离当前批次")?;
            if observed.receipt() != &sample.target_receipt
                || observed.descriptor() != &sample.target_descriptor
                || images.get(&sample.step) != Some(&self.image(&sample.item, observed)?)
            {
                return Err("对象图片向量、最新观察与本批原证据不同".into());
            }
            self.check_target_sample(observed)?;
        }
        Ok(())
    }

    pub(in crate::recovery::clone::native) fn write(
        &self,
        step: &Step<'_>,
        session: &mut Session<'_>,
        basis: &Basis,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        self.check_step(step)?;
        self.check_export()?;
        self.check_source(step.item)?;
        if basis != &self.pair(step.item)? {
            return Err("对象条件创建与原步骤前后像不同".into());
        }
        let directory = self.directory("transfer-write");
        object_write::create(
            &object_write::Create {
                backend: self.spec.backend,
                configuration_root: self.spec.target_configuration_root,
                artifact_root: self.spec.export_root,
                work: self.spec.work,
                directory: &directory,
                object: self.target(step.item),
                configuration: self.spec.target.configuration,
                generation: self.spec.generation_sha256,
                step: &step.id,
            },
            session,
            checkpoint,
        )
    }

    pub(in crate::recovery::clone::native) fn verify_source(
        &self,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Value> {
        self.check_all_source()?;
        let directory = self.directory("source-objects-final");
        let result = observe_source::observe(
            self.source_request(&directory),
            Selection::All,
            self.transport,
            checkpoint,
        )?;
        self.check_all_source()?;
        Ok(result)
    }

    pub(in crate::recovery::clone::native) fn verify_target(
        &self,
        items: &[Value],
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Value> {
        let target = &self.spec.target;
        verify_target_objects::verify(
            VerifyTarget {
                backend: self.spec.backend,
                configuration_root: self.spec.target_configuration_root,
                work: self.spec.work,
                directory: &self.directory("target-heads-final"),
                plan_sha256: self.spec.plan_sha256,
                generation_sha256: self.spec.generation_sha256,
                scope: target.scope,
                endpoint: target.endpoint,
                region: target.region,
                configuration: target.configuration,
                items,
                observations: &self.latest,
            },
            self.transport,
            checkpoint,
        )
    }

    pub(in crate::recovery::clone::native) fn verify_target_owners(
        &self,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Value> {
        checkpoint()?;
        let target = &self.spec.target;
        if target.configuration["app_env"] != "test"
            || app_configuration::public_configuration(
                self.spec.target_configuration_root,
                target.configuration,
            )? != json!({"endpoint":target.endpoint,"region":target.region})
        {
            return Err("目标五桶 owner 必须绑定已登记的 test 配置和端点区域".into());
        }
        let directory = self.directory("target-owners");
        common::ensure_new_directory(self.spec.backend, &directory, "目标五桶 owner 观察目录")?;
        let reads = ReadWindow::new(
            self.spec.backend,
            self.spec.target_configuration_root,
            &directory,
            target.scope,
            target.configuration,
            self.transport,
            checkpoint,
        )?;
        let result = (|| {
            let mut buckets = BUCKETS.to_vec();
            buckets.sort_unstable();
            let owners = reads.owners(&buckets, "owner")?;
            reads.configuration()?;
            let receipt = json!({"status":"target_owners_verified","scope_id":target.scope,
                "copy_plan_sha256":self.spec.plan_sha256,"generation_sha256":self.spec.generation_sha256,
                "endpoint":target.endpoint,"region":target.region,
                "configuration_sha256":digest::plan_hash(&json!({"configuration":target.configuration}))?,
                "owners":owners,"remote_writes":0});
            reads.write("verified.json", &receipt)?;
            reads.configuration()?;
            if document::read(&directory.join("verified.json"))? != receipt {
                return Err("目标五桶 owner 收据在发布后变化".into());
            }
            Ok(receipt)
        })();
        if result.is_err() {
            reads.write(
                "failure.json",
                &json!({"status":"target_owners_failed",
                "error_type":"TargetOwnersObservationError","remote_writes":0}),
            )?;
        }
        result
    }

    fn check_step(&self, step: &Step<'_>) -> Result<()> {
        document::exact(
            step.item,
            &["bucket", "source_key", "target_key", "artifact", "metadata"],
            "复制对象步骤",
        )?;
        let bucket = document::text(&step.item["bucket"], "复制对象桶")?;
        let source = document::text(&step.item["source_key"], "复制源对象 key")?;
        let target = document::text(&step.item["target_key"], "复制目标对象 key")?;
        validate_key(self.spec.source.scope, source, false)?;
        validate_key(self.spec.target.scope, target, false)?;
        let ids = copy_step_ids(
            &json!({"databases":[],"objects":[{"bucket":bucket,"target_key":target}]}),
        )?;
        if step.kind != Kind::Object
            || !BUCKETS.contains(&bucket)
            || ids["objects"][0] != step.id
            || step.item["artifact"]["sha256"] != step.artifact
            || source.split_once('/').map(|(_, key)| key)
                != target.split_once('/').map(|(_, key)| key)
        {
            return Err("对象步骤不属于已核精确源目标投影".into());
        }
        Ok(())
    }

    fn sample_source(
        &self,
        step: &Step<'_>,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<(Value, Value, Value)> {
        let (bucket, key) = item_key(step.item, "source_key")?;
        let directory = self.directory("source-object");
        let receipt = observe_source::observe(
            self.source_request(&directory),
            Selection::One {
                bucket: &bucket,
                key: &key,
            },
            self.transport,
            checkpoint,
        )?;
        let descriptor = common::binding(&directory.join("observation.json"))?;
        let files = tree::snapshot(self.spec.backend, &directory, false)?;
        Ok((receipt, descriptor, files))
    }

    fn sample_target(
        &self,
        step: &Step<'_>,
        checkpoint: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Observed> {
        let target = &self.spec.target;
        let (bucket, key) = item_key(step.item, "target_key")?;
        let artifact = &step.item["artifact"];
        observe_object::observe(
            ObserveTarget {
                backend: self.spec.backend,
                configuration_root: self.spec.target_configuration_root,
                work: self.spec.work,
                directory: &self.directory("target-object"),
                plan_sha256: self.spec.plan_sha256,
                scope: target.scope,
                endpoint: target.endpoint,
                region: target.region,
                configuration: target.configuration,
                bucket: &bucket,
                key: &key,
                expected: &json!({"bytes":artifact["bytes"],"sha256":artifact["sha256"]}),
                max_bytes: artifact["bytes"].as_u64().ok_or("计划对象字节数无效")?,
            },
            self.transport,
            checkpoint,
        )
    }

    fn source_request<'s>(&'s self, directory: &'s Path) -> ObserveSource<'s> {
        let side = &self.spec.source;
        ObserveSource {
            backend: self.spec.backend,
            work: self.spec.work,
            directory,
            export: self.spec.export,
            captures: self.spec.captures,
            scope: side.scope,
            endpoint: side.endpoint,
            region: side.region,
            configuration: side.configuration,
        }
    }

    fn target<'s>(&'s self, item: &'s Value) -> TargetObject<'s> {
        TargetObject {
            scope: self.spec.target.scope,
            endpoint: self.spec.target.endpoint,
            item,
        }
    }

    fn pair(&self, item: &Value) -> Result<Basis> {
        let (before, after) = images::object_pair(self.spec.generation_sha256, self.target(item))?;
        Ok(Basis { before, after })
    }

    fn image(&self, item: &Value, observed: &Observed) -> Result<Value> {
        images::object_observed(
            self.spec.generation_sha256,
            self.target(item),
            observed.image_evidence(),
        )
    }

    fn directory(&self, purpose: &str) -> PathBuf {
        let token = Uuid::new_v4().simple().to_string();
        self.spec.work.join(format!("{purpose}-{}", &token[..8]))
    }
}

fn item_key(item: &Value, field: &str) -> Result<Key> {
    Ok((
        document::text(&item["bucket"], "复制对象桶")?.to_owned(),
        document::text(&item[field], "复制对象 key")?.to_owned(),
    ))
}

fn batch<R: Send>(
    selected: &[&Step<'_>],
    read: impl Fn(&Step<'_>) -> Result<R> + Sync,
) -> Result<Vec<R>> {
    thread::scope(|scope| {
        let read = &read;
        let workers = selected
            .iter()
            .map(|step| {
                scope.spawn(move || read(step).map_err(|_| "对象批次读取失败，保留本次阶段证据"))
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or(Err("对象观察线程未完成")))
            .collect::<Vec<_>>()
    })
    .into_iter()
    .map(|result| result.map_err(Into::into))
    .collect()
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/object_port.rs"]
mod tests;

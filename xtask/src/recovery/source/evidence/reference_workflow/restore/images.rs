//! 在同一目标窗口采集、复核并发布数据库、对象和 Redis 的整体像。
use std::path::Path;

use serde_json::{Map, Value, json};

use super::{Execution, Phase, combine, database_images, native, object_images, text};
use crate::{
    Result,
    recovery::inputs::document::{self, Document},
};
use native::{
    digest::plan_hash, export::transport::SystemTransport, resources::common,
    target_verify::guard::Inputs,
};

pub(super) fn capture(
    window: &Execution<'_>,
    phase: Phase,
    expected: &Value,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
    evidence: &mut Map<String, Value>,
) -> Result<()> {
    let side = text(&window.snapshot.target.value["target_side"])?;
    let backend = window.stage.backend;
    let directory = window
        .stage
        .work
        .join(format!("restore-{side}-{}", phase.name()));
    record(backend, &directory, phase, evidence, || {
        observe(window, phase, &directory, expected, checkpoint)
    })
}

fn record(
    backend: &Path,
    directory: &Path,
    phase: Phase,
    evidence: &mut Map<String, Value>,
    observe: impl FnOnce() -> Result<(bool, Value)>,
) -> Result<()> {
    common::ensure_new_directory(backend, directory, "参考恢复资源像目录")?;
    let name = phase.name();
    let result = (|| {
        let (matches, binding) = observe()?;
        evidence.insert(name.to_owned(), binding);
        if !matches && phase != Phase::FailureAfter {
            return Err("恢复完整资源像与预期不同，保留全部采集结果".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let saved = publish(
            backend,
            &directory.join("failure.json"),
            &json!({"phase":name,"error_type":"ReferenceResourceImageFailure"}),
        )
        .map(|binding| {
            evidence.insert(format!("{name}-failure"), binding);
        });
        return combine(Err(error), saved);
    }
    Ok(())
}

fn observe(
    window: &Execution<'_>,
    phase: Phase,
    directory: &Path,
    expected: &Value,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
) -> Result<(bool, Value)> {
    let target = window.target;
    let snapshot = window.snapshot;
    let storage = target.inputs();
    let reference = &snapshot.chain.reference.value;
    let initialized = target.initialized();
    let configuration = target.configuration();
    let database = database_images::capture(
        &database_images::Capture {
            storage: &storage,
            reference,
            initialized,
            configuration,
            scope: window.scope,
            initialized_directory: target.initialized_directory(),
            phase_root: directory,
        },
        &SystemTransport,
        checkpoint,
    )?;
    let objects = &reference["target"]["s3"];
    let (objects, object_proofs) = object_images::capture(
        &object_images::Observe {
            storage: &storage,
            configuration_root: &storage.execution_backend()?,
            configuration,
            directory,
            scope: text(&reference["target"]["scope_id"])?,
            endpoint: text(&objects["endpoint"])?,
            region: text(&objects["region"])?,
            manifest: &snapshot.manifest.value,
            expected: &expected["objects"],
            plan_sha256: &plan_hash(reference)?,
            generation_sha256: &plan_hash(&initialized["generation"])?,
        },
        phase,
        &SystemTransport,
        checkpoint,
    )?;
    Publication {
        backend: window.stage.backend,
        directory,
        target: &snapshot.target,
        phase,
        expected,
    }
    .publish(
        Observations {
            database,
            objects,
            object_proofs,
        },
        || target.redis_state(),
        checkpoint,
    )
}

struct Observations<'a> {
    database: database_images::Observed<'a>,
    objects: Value,
    object_proofs: Vec<Value>,
}

struct Publication<'a> {
    backend: &'a Path,
    directory: &'a Path,
    target: &'a Document,
    phase: Phase,
    expected: &'a Value,
}

impl Publication<'_> {
    fn publish(
        self,
        observed: Observations<'_>,
        redis: impl FnOnce() -> Result<Value>,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<(bool, Value)> {
        let Observations {
            database,
            objects,
            object_proofs,
        } = observed;
        let proof_paths = object_proofs
            .iter()
            .map(|proof| document::binding_shape(proof, "参考恢复对象证明").map(|file| file.path))
            .collect::<Result<Vec<_>>>()?;
        let protected = Inputs::open(proof_paths)?;
        let redis = redis()?;
        database.check(checkpoint)?;
        for proof in &object_proofs {
            let (_, file) = document::bound_file(self.backend, proof, "参考恢复对象证明")?;
            file.assert_unchanged()?;
        }
        checkpoint()?;
        let actual = json!({"databases":database.observations,"objects":objects,"redis":redis});
        let matches = actual == *self.expected;
        let image = json!({
            "format_version":1,"kind":"restore-reference-resource-image","phase":self.phase.name(),
            "target_plan":document::binding(self.target),"inventory":database.receipt,
            "object_observations":object_proofs,"image":actual,"expected_sha256":plan_hash(self.expected)?,
            "matches_expected":matches,
        });
        let binding = publish(self.backend, &self.directory.join("image.json"), &image)?;
        protected.check()?;
        checkpoint()?;
        Ok((matches, binding))
    }
}

fn publish(backend: &Path, path: &Path, value: &Value) -> Result<Value> {
    native::json::write_new(path, value)?;
    let published = document::read(path, backend, "参考恢复资源像收据")?;
    if published.value != *value {
        return Err("发布的参考恢复资源像收据发生变化".into());
    }
    published.assert_unchanged()?;
    Ok(document::binding(&published))
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/images_tests.rs"]
mod tests;

//! 在同一目标窗口采集、复核并发布数据库、对象和 Redis 的整体像。
use std::path::Path;

use serde_json::{Map, Value, json};

use super::{Execution, combine, database_images, native, object_images, text};
use crate::{Result, recovery::inputs::document};
use native::{
    digest::plan_hash, export::transport::SystemTransport, resources::common,
    target_verify::guard::Inputs,
};

pub(super) fn capture(
    window: &Execution<'_>,
    phase: &str,
    expected: &Value,
    checkpoint: &(dyn Fn() -> Result<()> + Sync),
    evidence: &mut Map<String, Value>,
) -> Result<()> {
    let side = text(&window.snapshot.target.value["target_side"])?;
    let backend = window.stage.backend;
    let directory = window.stage.work.join(format!("restore-{side}-{phase}"));
    common::ensure_new_directory(backend, &directory, "参考恢复资源像目录")?;
    let result = (|| {
        let (matches, binding) = observe(window, phase, &directory, expected, checkpoint)?;
        evidence.insert(phase.to_owned(), binding);
        if !matches {
            return Err("恢复完整资源像与预期不同，保留全部采集结果".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let saved = publish(
            backend,
            &directory.join("failure.json"),
            &json!({"phase":phase,"error_type":"ReferenceResourceImageFailure"}),
        )
        .map(|binding| {
            evidence.insert(format!("{phase}-failure"), binding);
        });
        return combine(Err(error), saved);
    }
    Ok(())
}

fn observe(
    window: &Execution<'_>,
    phase: &str,
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
        phase != "before",
        &SystemTransport,
        checkpoint,
    )?;
    let proof_paths = object_proofs
        .iter()
        .map(|proof| document::binding_shape(proof, "参考恢复对象证明").map(|file| file.path))
        .collect::<Result<Vec<_>>>()?;
    let protected = Inputs::open(proof_paths)?;
    let redis = target.redis_state()?;
    database.check(checkpoint)?;
    for proof in &object_proofs {
        let (_, file) = document::bound_file(window.stage.backend, proof, "参考恢复对象证明")?;
        file.assert_unchanged()?;
    }
    checkpoint()?;
    let actual = json!({"databases":database.observations,"objects":objects,"redis":redis});
    let matches = actual == *expected;
    let image = json!({
        "format_version":1,"kind":"restore-reference-resource-image","phase":phase,
        "target_plan":document::binding(&snapshot.target),"inventory":database.receipt,
        "object_observations":object_proofs,"image":actual,"expected_sha256":plan_hash(expected)?,
        "matches_expected":matches,
    });
    let binding = publish(window.stage.backend, &directory.join("image.json"), &image)?;
    protected.check()?;
    checkpoint()?;
    Ok((matches, binding))
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

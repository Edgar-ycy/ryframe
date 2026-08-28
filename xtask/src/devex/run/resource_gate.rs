use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Result;

use super::super::{
    model::{RESOURCE_GATE_DECISION_TEMPLATE, RESOURCE_GATE_TARGETED_ACTIVATION},
    report::ResourceGateDecisionEvidence,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResourceGateDecisionArtifact {
    format_version: u16,
    recognized: bool,
    mode: String,
    fallback: Option<String>,
    steps: Vec<String>,
}

pub(crate) fn read_resource_gate_decision(
    target: &Path,
    label: &str,
) -> Result<ResourceGateDecisionEvidence> {
    let path = decision_path(target, label);
    let document = fs::read(&path).map_err(|error| {
        format!(
            "resource-gate 缺少 decision artifact {}：{error}",
            path.display()
        )
    })?;
    let artifact: ResourceGateDecisionArtifact = serde_json::from_slice(&document)
        .map_err(|error| format!("resource-gate decision artifact 无效：{error}"))?;
    validate(&artifact)?;
    Ok(ResourceGateDecisionEvidence {
        format_version: artifact.format_version,
        recognized: artifact.recognized,
        mode: artifact.mode,
        fallback: artifact.fallback,
        steps: artifact.steps,
        artifact_sha256: format_sha256(Sha256::digest(&document)),
    })
}

fn validate(artifact: &ResourceGateDecisionArtifact) -> Result<()> {
    if artifact.format_version == 1
        && artifact.recognized
        && artifact.mode == "targeted"
        && artifact.fallback.is_none()
        && !artifact.steps.is_empty()
        && artifact.steps.iter().all(|step| !step.trim().is_empty())
    {
        return Ok(());
    }
    Err(format!(
        "resource-gate decision 未证明 {} targeted 模式：recognized={} mode={} fallback={:?}",
        RESOURCE_GATE_TARGETED_ACTIVATION, artifact.recognized, artifact.mode, artifact.fallback
    )
    .into())
}

fn decision_path(target: &Path, label: &str) -> PathBuf {
    PathBuf::from(
        RESOURCE_GATE_DECISION_TEMPLATE
            .replace("{target}", &target.to_string_lossy())
            .replace("{label}", label),
    )
}

fn format_sha256(digest: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let mut value = String::from("sha256:");
    for byte in digest.as_ref() {
        write!(value, "{byte:02x}").expect("写入 String 不会失败");
    }
    value
}

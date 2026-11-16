use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Result;

use super::super::{
    model::{PairedArm, RESOURCE_GATE_DECISION_TEMPLATE, RESOURCE_GATE_TARGETED_ACTIVATION},
    report::ResourceGateDecisionEvidence,
    targeted_source::TargetedArm,
};

const LEGACY_GATE_ENVIRONMENT: [&str; 8] = [
    "RYFRAME_RESOURCE_GATE_TARGETED",
    "RYFRAME_RESOURCE_GATE_DECISION_FILE",
    "RYFRAME_CI_BASE_SHA",
    "RYFRAME_CI_HEAD_SHA",
    "RYFRAME_VERIFY_JOBS",
    "RYFRAME_RESOURCE_GATE_TEST_JOBS",
    "RYFRAME_WORKSPACE_ROOT",
    "RYFRAME_DEVEX_TARGET_ROOT",
];

pub(super) fn bind(
    command: &mut Command,
    backend: &Path,
    target: &Path,
    label: &str,
    arm: PairedArm,
    source: &TargetedArm,
) -> Result<()> {
    verify_cache(backend)?;
    command.current_dir(super::super::support::child_path(backend));
    for key in LEGACY_GATE_ENVIRONMENT {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
    if arm == PairedArm::Baseline {
        command
            .env(
                "RYFRAME_RESOURCE_GATE_TARGETED",
                RESOURCE_GATE_TARGETED_ACTIVATION,
            )
            .env(
                "RYFRAME_RESOURCE_GATE_DECISION_FILE",
                decision_path(target, label),
            )
            .env("RYFRAME_CI_BASE_SHA", &source.backend_base)
            .env("RYFRAME_CI_HEAD_SHA", &source.backend_head)
            .env("RYFRAME_VERIFY_JOBS", "12")
            .env("RYFRAME_RESOURCE_GATE_TEST_JOBS", "8");
    } else {
        command.args(source.candidate_arguments());
    }
    Ok(())
}

pub(super) fn check_new_decision(target: &Path, label: &str) -> Result<()> {
    if decision_path(target, label).try_exists()? {
        return Err("resource-gate 样本已有决策文件，拒绝覆盖".into());
    }
    Ok(())
}

pub(super) fn sample_decision(
    target: &Path,
    label: &str,
    child_succeeded: bool,
    strict: Option<&[String]>,
) -> Result<(Option<ResourceGateDecisionEvidence>, bool)> {
    let Some(steps) = strict else {
        let decision = child_succeeded
            .then(|| read_resource_gate_decision(target, label))
            .transpose()?;
        return Ok((decision, true));
    };
    match read_resource_gate_decision(target, label) {
        Ok(decision) => {
            let matched = decision.matches_plan(steps);
            if !matched {
                eprintln!("resource-gate 样本 {label} 的实际决策偏离定向计划：{decision:?}");
            }
            Ok((Some(decision), matched))
        }
        Err(error) => {
            eprintln!("resource-gate 样本 {label} 的决策证据无法确认：{error}");
            Ok((None, false))
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/internal/devex_targeted_gate.rs"]
mod tests;

pub(in crate::devex) fn verify_cache(backend: &Path) -> Result<()> {
    for directory in [
        "target",
        "target/ci",
        "target/ci/backend",
        "target/ci/resource",
    ] {
        let path = backend.join(directory);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !linked(&metadata) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => {
                return Err(format!(
                    "resource-gate 缓存必须在实际后端工作树内：{}",
                    path.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

pub(in crate::devex) fn lock_cache(backend: &Path) -> Result<File> {
    verify_cache(backend)?;
    let directory = backend.join("target/ci");
    fs::create_dir_all(&directory)?;
    let path = directory.join("devex-resource-gate.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !linked(&metadata) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err("resource-gate 缓存锁文件必须是普通文件".into()),
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.try_lock()
        .map_err(|error| format!("resource-gate 无法取得缓存锁 {}：{error}", path.display()))?;
    Ok(file)
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

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
    let valid_mode =
        (artifact.recognized && artifact.mode == "targeted" && artifact.fallback.is_none())
            || (artifact.mode == "full"
                && artifact
                    .fallback
                    .as_ref()
                    .is_some_and(|reason| !reason.trim().is_empty()));
    if artifact.format_version == 1
        && valid_mode
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

pub(super) fn decision_path(target: &Path, label: &str) -> PathBuf {
    target.join(
        RESOURCE_GATE_DECISION_TEMPLATE
            .trim_start_matches("{target}/")
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

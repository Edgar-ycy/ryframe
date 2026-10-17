use std::{fs, time::Instant};

use serde_json::{Map, Value, json};

use crate::{Result, workspace::root_dir};

use super::super::{
    archive::{read_archive, validate_archive_evidence},
    github::Api,
};

use super::collect::{api_timeout, pages};

pub(crate) const ARTIFACT_ARCHIVE_MAX_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) fn pair_receipts(
    api: &dyn Api,
    evidence: &Value,
    backend_sha: &str,
    frontend_sha: &str,
    fixture_sha256: &str,
    deadline: Instant,
) -> Result<Value> {
    let repository = evidence
        .get("repository")
        .and_then(Value::as_str)
        .ok_or_else(|| "扩展 CI 证据缺少 repository".to_string())?;
    let run_id = evidence
        .get("run_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "扩展 CI 证据缺少 run_id".to_string())?;
    let attempt = evidence
        .get("attempt")
        .and_then(Value::as_u64)
        .ok_or_else(|| "扩展 CI 证据缺少 attempt".to_string())?;
    let prefix = format!("repos/{repository}/actions");
    let artifacts = pages(
        api,
        &format!("{prefix}/runs/{run_id}/artifacts"),
        "artifacts",
        deadline,
    )?;
    let selected = select_artifacts(&artifacts, run_id, attempt)?;
    let temporary_root = root_dir().join(".local-tests");
    fs::create_dir_all(&temporary_root)
        .map_err(|error| format!("无法创建发布证据临时目录：{error}"))?;
    let temporary = tempfile::Builder::new()
        .prefix("ryframe-release-evidence-")
        .tempdir_in(&temporary_root)
        .map_err(|error| format!("无法创建发布证据临时目录：{error}"))?;
    let mut result = Map::new();
    for (fixture, name, artifact, size) in selected {
        let archive_path = temporary.path().join(format!("{fixture}.zip"));
        let endpoint = format!("{prefix}/artifacts/{}/zip", artifact["id"]);
        api.download(
            &endpoint,
            &archive_path,
            api_timeout(deadline, Instant::now())?,
        )?;
        let actual_size = fs::metadata(&archive_path)
            .map_err(|error| format!("无法读取证据 ZIP 元数据：{error}"))?
            .len();
        if actual_size != size || actual_size > ARTIFACT_ARCHIVE_MAX_BYTES {
            return Err("全栈证据 ZIP 实际大小与受控元数据不一致".into());
        }
        let entries = read_archive(&archive_path)?;
        let validated = validate_archive_evidence(
            &entries,
            &fixture,
            backend_sha,
            frontend_sha,
            run_id,
            attempt,
            (fixture == "business").then_some(fixture_sha256),
        )?;
        let mut receipt = validated["source_pair"]
            .as_object()
            .cloned()
            .ok_or_else(|| "全栈源码组合收据不是对象".to_string())?;
        receipt.insert(
            "archive_evidence".to_owned(),
            json!({
                "build_evidence": validated["build_evidence"],
                "runtime": validated["runtime"],
                "runtime_evidence": validated["runtime_evidence"],
            }),
        );
        receipt.insert(
            "artifact".to_owned(),
            json!({
                "id": artifact["id"],
                "name": name,
                "size_in_bytes": size,
            }),
        );
        if fixture == "business" {
            receipt.insert(
                "fixture".to_owned(),
                validated
                    .get("fixture_receipt")
                    .cloned()
                    .ok_or_else(|| "业务 crate fixture 收据缺失".to_string())?,
            );
        }
        result.insert(fixture, Value::Object(receipt));
    }
    Ok(Value::Object(result))
}

pub(crate) fn select_artifacts(
    artifacts: &[Value],
    run_id: u64,
    attempt: u64,
) -> Result<Vec<(String, String, Value, u64)>> {
    let mut selected = Vec::new();
    for (fixture, suffix) in [("core", ""), ("business", "-business")] {
        let name = format!("ryframe-full-stack-{run_id}-{attempt}{suffix}");
        let matches = artifacts
            .iter()
            .filter(|artifact| {
                artifact.get("name").and_then(Value::as_str) == Some(name.as_str())
                    && artifact.get("expired").and_then(Value::as_bool) != Some(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(format!("真实全栈 {fixture} 证据产物缺失、重复或过期").into());
        }
        let artifact = matches.into_iter().next().unwrap_or(Value::Null);
        artifact
            .get("id")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or_else(|| format!("真实全栈 {fixture} 证据产物缺少有效 ID"))?;
        let size = artifact_size(&artifact)?;
        selected.push((fixture.to_owned(), name, artifact, size));
    }
    Ok(selected)
}

fn artifact_size(artifact: &Value) -> Result<u64> {
    artifact
        .get("size_in_bytes")
        .and_then(Value::as_u64)
        .filter(|size| *size > 0 && *size <= ARTIFACT_ARCHIVE_MAX_BYTES)
        .ok_or_else(|| "全栈证据 ZIP 缺少有效大小或超过下载硬上限".into())
}

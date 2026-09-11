use ryframe_application::ports::backup::RestoreBusinessProof;
use ryframe_kernel::{AppError, AppResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

const MAX_RECEIPT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreTestRun {
    title: Vec<String>,
    status: String,
    retry: u32,
    scenarios: Vec<String>,
}

pub fn verify_tests_receipt(proof: &RestoreBusinessProof, path: &Path) -> AppResult<()> {
    let bytes = read_regular_file(path)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != proof.tests_receipt_sha256 {
        return Err(validation("恢复测试明细摘要与业务证明不一致"));
    }
    let runs: Vec<RestoreTestRun> = serde_json::from_slice(&bytes)
        .map_err(|error| validation(format!("恢复测试明细格式无效：{error}")))?;
    validate_runs(proof, &runs)
}

fn read_regular_file(path: &Path) -> AppResult<Vec<u8>> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| validation("恢复测试明细无法读取"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err(validation("恢复测试明细必须是 16 MiB 内的非空普通文件"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_RECEIPT_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| validation("恢复测试明细无法读取"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(validation("恢复测试明细读取期间发生变化"));
    }
    Ok(bytes)
}

fn validate_runs(proof: &RestoreBusinessProof, runs: &[RestoreTestRun]) -> AppResult<()> {
    let mut observed = BTreeSet::new();
    let valid_runs = !runs.is_empty()
        && runs.iter().all(|run| {
            !run.title.is_empty()
                && run.title.iter().all(|part| !part.trim().is_empty())
                && run.status == "passed"
                && run.retry == 0
                && run
                    .scenarios
                    .iter()
                    .all(|scenario| !scenario.is_empty() && observed.insert(scenario.as_str()))
        });
    let expected = proof
        .scenarios
        .iter()
        .filter(|scenario| scenario.succeeded)
        .map(|scenario| scenario.name.as_str())
        .collect::<BTreeSet<_>>();
    if !valid_runs || observed != expected || observed.len() != proof.scenarios.len() {
        return Err(validation(
            "恢复测试明细存在失败、重试、重复或与业务证明不同的场景",
        ));
    }
    Ok(())
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use ryframe_application::ports::backup::RestoreScenarioResult;
    use std::io::Write;

    fn proof(bytes: &[u8]) -> RestoreBusinessProof {
        RestoreBusinessProof {
            restore_id: "restore-test".into(),
            plan_hash: "a".repeat(64),
            backend_sha: "b".repeat(40),
            frontend_sha: "c".repeat(40),
            runner_sha: "d".repeat(40),
            scope_id: "restore-test".into(),
            runtime_receipt_sha256: "e".repeat(64),
            tests_receipt_sha256: hex::encode(Sha256::digest(bytes)),
            started_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            completed_at: Utc.timestamp_opt(1_800_000_001, 0).unwrap(),
            scenarios: ["login", "post"]
                .map(|name| RestoreScenarioResult {
                    name: name.into(),
                    succeeded: true,
                })
                .to_vec(),
            unexpected_console_messages: 0,
            unexpected_network_failures: 0,
            axe_serious_or_critical: 0,
        }
    }

    fn receipt(directory: &Path, bytes: &[u8]) -> std::path::PathBuf {
        let path = directory.join("tests.json");
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    #[test]
    fn exact_test_receipt_is_bound_to_proof() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = br#"[{"title":["full stack"],"status":"passed","retry":0,"scenarios":["login"]},{"title":["post"],"status":"passed","retry":0,"scenarios":["post"]}]"#;
        verify_tests_receipt(&proof(bytes), &receipt(directory.path(), bytes)).unwrap();
    }

    #[test]
    fn changed_or_incomplete_test_receipt_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let cases: &[&[u8]] = &[
            br#"[{"title":["full stack"],"status":"failed","retry":0,"scenarios":["login","post"]}]"#,
            br#"[{"title":["full stack"],"status":"passed","retry":1,"scenarios":["login","post"]}]"#,
            br#"[{"title":["full stack"],"status":"passed","retry":0,"scenarios":["login","login"]}]"#,
            br#"[{"title":["full stack"],"status":"passed","retry":0,"scenarios":["login"]}]"#,
        ];
        for (index, bytes) in cases.iter().enumerate() {
            let path = directory.path().join(format!("tests-{index}.json"));
            File::create(&path).unwrap().write_all(bytes).unwrap();
            assert!(
                verify_tests_receipt(&proof(bytes), &path).is_err(),
                "case {index}"
            );
        }
        let changed =
            br#"[{"title":["changed"],"status":"passed","retry":0,"scenarios":["login","post"]}]"#;
        let path = receipt(directory.path(), changed);
        let original =
            br#"[{"title":["original"],"status":"passed","retry":0,"scenarios":["login","post"]}]"#;
        assert!(verify_tests_receipt(&proof(original), &path).is_err());
    }
}

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    devex::{BaselineContract, DevexSuite, PairedArm, compare, summarize},
    devex_tests::{clean_source_fingerprint, fake_stable_frontend_fast_run},
};

#[test]
fn embedded_stable_readiness_adapter_has_registered_digest_and_scope() {
    let patch = BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH;
    assert_eq!(
        format!("sha256:{}", hex::encode(Sha256::digest(patch))),
        BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
    );
    let patch = std::str::from_utf8(patch).unwrap();
    for path in BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS {
        assert!(patch.contains(&format!("diff --git a/{path} b/{path}")));
    }
    assert_eq!(
        patch.matches("diff --git ").count(),
        BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS.len()
    );
}

#[test]
fn common_toolchain_report_binds_frontend_adaptation_and_exact_runner() {
    let baseline = fake_stable_frontend_fast_run("common-fast-b0", PairedArm::Baseline);
    let candidate = fake_stable_frontend_fast_run("common-fast-b1", PairedArm::Candidate);
    let contract = BaselineContract::StableReadinessB0CommonToolchainV1;
    let workload = contract
        .workload_contract(DevexSuite::FrontendFast, "default")
        .unwrap();
    let original = BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT;
    let adaptation = contract.registered_frontend_adaptation().unwrap();
    let expected = serde_json::to_value(&adaptation).unwrap();
    let fingerprint = clean_source_fingerprint(&adaptation.commit);
    let baseline_path = baseline.join("metadata.json");
    let candidate_path = candidate.join("metadata.json");
    let mut baseline_metadata = json_file(&baseline_path);
    let mut candidate_metadata = json_file(&candidate_path);
    let toolchain = common_toolchain_fixture();
    let runner = BaselineContract::COMMON_FRONTEND_RUNNER_COMMIT;
    let runner_fingerprint = clean_source_fingerprint(runner);
    for (metadata, directory, arm) in [
        (&mut baseline_metadata, &baseline, "b0"),
        (&mut candidate_metadata, &candidate, "b1"),
    ] {
        metadata["pairing"]["baseline_contract"] = json!("stable-readiness-b0-common-toolchain-v1");
        metadata["pairing"]["baseline_provenance"]["frontend_adaptation"] = expected.clone();
        metadata["pairing"]["workload_contract"] = json!(workload);
        metadata["toolchain"]["common"] = toolchain.clone();
        metadata["runner_frontend"] = json!({
            "commit": runner, "dirty": false, "worktree_fingerprint": runner_fingerprint,
        });
        if arm == "b0" {
            metadata["frontend"]["commit"] = json!(adaptation.commit);
            metadata["frontend"]["worktree_fingerprint"] = json!(fingerprint);
        }
        bind_frontend_sample_fingerprints(directory, metadata);
        metadata["commands"] = json!([{
            "program": "node", "working_directory": "$FRONTEND",
            "args": ["$RUNNER_FRONTEND/scripts/devex-fast.mjs", "--root", "$FRONTEND",
                     "--arm", arm, "--cache-root", "$FRONTEND_CACHE"],
        }]);
    }
    write_json(&baseline_path, &baseline_metadata);
    write_json(&candidate_path, &candidate_metadata);

    compare(&baseline, &candidate).unwrap();
    let report = json_file(&candidate.join("comparison.json"));
    assert_eq!(report["baselineProvenance"]["frontend_commit"], original);
    assert_eq!(
        report["baselineProvenance"]["frontend_adaptation"],
        expected
    );
    assert_eq!(report["commonToolchain"], toolchain);
    assert_eq!(workload.baseline_primitives.len(), 9);
    assert_eq!(workload.candidate_primitives.len(), 8);

    let mut changed = candidate_metadata.clone();
    changed["runner_frontend"]["commit"] = json!(original);
    write_json(&candidate_path, &changed);
    assert!(
        compare(&baseline, &candidate)
            .unwrap_err()
            .to_string()
            .contains("工具来源不匹配")
    );
    let mut changed = candidate_metadata.clone();
    changed["toolchain"]["common"] = Value::Null;
    write_json(&candidate_path, &changed);
    assert!(
        compare(&baseline, &candidate)
            .unwrap_err()
            .to_string()
            .contains("两侧一致的工具链证据")
    );
    write_json(&candidate_path, &candidate_metadata);

    reject_changed_b0_frontend(&baseline, &candidate, &baseline_metadata);
    for (key, value) in [
        ("working_directory", json!("$RUNNER_FRONTEND")),
        ("program", json!("corepack")),
        ("args", json!(["pnpm", "check"])),
    ] {
        let mut changed = candidate_metadata.clone();
        changed["commands"][0][key] = value;
        write_json(&candidate_path, &changed);
        let error = compare(&baseline, &candidate).unwrap_err().to_string();
        assert!(error.contains("实际执行入口不匹配"), "{key}: {error}");
    }
    for path in [baseline, candidate] {
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn common_toolchain_windows_workspace_reports_require_four_actual_cargo_jobs() {
    for (suite, operation, cache) in [
        (DevexSuite::RustColdBuild, "build", "cold"),
        (DevexSuite::RustIncremental, "check", "warm"),
        (DevexSuite::RustSccache, "check", "warm"),
    ] {
        for arm in [PairedArm::Baseline, PairedArm::Candidate] {
            let (directory, metadata) = common_workspace_run(suite, operation, cache, arm);
            let path = directory.join("metadata.json");
            write_json(&path, &metadata);
            assert_eq!(summarize(&directory).unwrap().passed, 6);

            let mut changed = metadata.clone();
            changed["jobs"] = json!(8);
            write_json(&path, &changed);
            assert!(
                summarize(&directory)
                    .unwrap_err()
                    .to_string()
                    .contains("metadata.jobs 必须为 4")
            );

            for (field, value) in [
                (
                    "args",
                    json!([operation, "--locked", "--workspace", "--jobs", "8"]),
                ),
                ("args", json!([operation, "--locked", "--workspace"])),
                (
                    "args",
                    json!([
                        if operation == "build" {
                            "check"
                        } else {
                            "build"
                        },
                        "--locked",
                        "--workspace",
                        "--jobs",
                        "4"
                    ]),
                ),
                ("program", json!("rustup")),
                ("working_directory", json!("$FRONTEND")),
            ] {
                let mut changed = metadata.clone();
                changed["commands"][0][field] = value;
                write_json(&path, &changed);
                let error = summarize(&directory).unwrap_err().to_string();
                assert!(
                    error.contains("要求实际 Cargo 命令"),
                    "{suite:?} {arm:?} {field}: {error}"
                );
            }
            let mut changed = metadata.clone();
            changed["commands"] = json!([metadata["commands"][0], metadata["commands"][0]]);
            write_json(&path, &changed);
            assert!(
                summarize(&directory)
                    .unwrap_err()
                    .to_string()
                    .contains("唯一的 Cargo 命令")
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn workspace_report_jobs_rule_keeps_other_targets_variants_and_contracts() {
    let (directory, metadata) = common_workspace_run(
        DevexSuite::RustColdBuild,
        "build",
        "cold",
        PairedArm::Baseline,
    );
    let path = directory.join("metadata.json");
    let mut legacy = metadata["pairing"].clone();
    legacy["baseline_contract"] = json!(BaselineContract::LegacyStableReadinessB0V1.as_str());
    legacy["baseline_provenance"]["frontend_adaptation"] = Value::Null;
    for (field, value) in [
        ("target", json!("x86_64-unknown-linux-gnu")),
        ("variant", json!("api")),
        ("pairing", legacy),
        ("pairing", Value::Null),
    ] {
        let mut changed = metadata.clone();
        changed["jobs"] = json!(8);
        changed["commands"] = json!([]);
        changed[field] = value;
        write_json(&path, &changed);
        assert_eq!(summarize(&directory).unwrap().passed, 6);
    }
    fs::remove_dir_all(directory).unwrap();
}

fn common_workspace_run(
    suite: DevexSuite,
    operation: &str,
    cache: &str,
    arm: PairedArm,
) -> (PathBuf, Value) {
    let directory = fake_stable_frontend_fast_run(suite.as_str(), arm);
    let mut metadata = json_file(&directory.join("metadata.json"));
    let contract = BaselineContract::StableReadinessB0CommonToolchainV1;
    metadata["suite"] = json!(suite);
    metadata["variant"] = json!("workspace");
    metadata["cache_state"] = json!(cache);
    metadata["features"] = json!(["workspace-default"]);
    metadata["frontend"] = Value::Null;
    metadata["pairing"]["baseline_contract"] = json!(contract.as_str());
    metadata["pairing"]["baseline_provenance"]["frontend_adaptation"] =
        json!(contract.registered_frontend_adaptation().unwrap());
    metadata["pairing"]["workload_contract"] = Value::Null;
    metadata["toolchain"]["common"] = common_toolchain_fixture();
    metadata["commands"] = json!([{
        "program": "cargo", "working_directory": "$BACKEND",
        "args": [operation, "--locked", "--workspace", "--jobs", "4"],
    }]);
    let samples = directory.join("samples.jsonl");
    let original = fs::read_to_string(&samples).unwrap();
    let mut updated = String::new();
    for line in original.lines() {
        let mut sample: Value = serde_json::from_str(line).unwrap();
        sample["cache_state"] = json!(cache);
        sample["source_fingerprints"]["frontend"] = Value::Null;
        updated.push_str(&sample.to_string());
        updated.push('\n');
    }
    fs::write(samples, updated).unwrap();
    if suite == DevexSuite::RustSccache {
        let snapshot = json!({"stats": {
            "compile_requests": 0, "requests_not_cacheable": 0, "not_cached": {},
            "cache_hits": {"counts": {}}, "cache_misses": {"counts": {}},
            "cache_errors": {"counts": {}}, "cache_timeouts": 0, "cache_read_errors": 0,
            "cache_write_errors": 0, "dist_errors": 0,
        }});
        for name in ["sccache-before.json", "sccache-after.json"] {
            write_json(&directory.join(name), &snapshot);
        }
    }
    (directory, metadata)
}

fn reject_changed_b0_frontend(baseline: &Path, candidate: &Path, metadata: &Value) {
    let metadata_path = baseline.join("metadata.json");
    for field in [
        "original_tree",
        "commit",
        "tree",
        "patch_sha256",
        "application_lock_sha256",
        "manager_lock_sha256",
    ] {
        let mut changed = metadata.clone();
        changed["pairing"]["baseline_provenance"]["frontend_adaptation"][field] = json!("changed");
        write_json(&metadata_path, &changed);
        let error = compare(baseline, candidate).unwrap_err().to_string();
        assert!(error.contains("来源或执行适配证据无效"), "{field}: {error}");
    }
    let mut changed = metadata.clone();
    changed["pairing"]["baseline_provenance"]["frontend_adaptation"] = Value::Null;
    write_json(&metadata_path, &changed);
    assert!(
        compare(baseline, candidate)
            .unwrap_err()
            .to_string()
            .contains("来源或执行适配证据无效")
    );
    let mut changed = metadata.clone();
    changed["frontend"]["commit"] = json!(BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT);
    write_json(&metadata_path, &changed);
    assert!(
        compare(baseline, candidate)
            .unwrap_err()
            .to_string()
            .contains("实际前端 B0 身份不一致")
    );
    write_json(&metadata_path, metadata);
}

fn bind_frontend_sample_fingerprints(directory: &Path, metadata: &Value) {
    let path = directory.join("samples.jsonl");
    let original = fs::read_to_string(&path).unwrap();
    let mut updated = String::new();
    for line in original.lines() {
        let mut sample: Value = serde_json::from_str(line).unwrap();
        for source in ["frontend", "runner_frontend"] {
            sample["source_fingerprints"][source] =
                metadata[source]["worktree_fingerprint"].clone();
        }
        updated.push_str(&sample.to_string());
        updated.push('\n');
    }
    fs::write(path, updated).unwrap();
}

fn common_toolchain_fixture() -> Value {
    let file = |name: &str| {
        let path = format!("tools/{name}");
        json!({
            "path": path,
            "canonical_path": path,
            "bytes": 12,
            "modified": std::time::UNIX_EPOCH,
            "sha256": "a".repeat(64),
        })
    };
    let versioned = |name, version| json!({ "file": file(name), "version": version });
    json!({
        "rustup_toolchain": "1.98.1",
        "rustup": file("rustup"),
        "cargo": {
            "proxy_path": "tools/cargo", "proxy_copy": null,
            "actual": versioned("cargo", "cargo 1.98.1"),
        },
        "rustc": {
            "proxy_path": "tools/rustc", "proxy_copy": null,
            "actual": versioned("rustc", "rustc 1.98.1"),
        },
        "node": versioned("node", "v24.21.0"),
        "corepack": versioned("corepack", "0.34.0"),
        "pnpm": versioned("pnpm", "12.8.1"),
        "pnpm_node": "tools/node",
        "corepack_home": "tools",
        "pnpm_cache_dir": null,
    })
}

fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

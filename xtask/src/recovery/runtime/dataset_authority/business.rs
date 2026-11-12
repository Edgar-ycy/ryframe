//! 从产品维护程序提供的数据库权威独立复核最终浏览器证明。
use std::path::Path;

use serde_json::Value;

use super::{check, inputs::Inputs, path, reads::Window, verified_target::VerifiedTarget};
use crate::{
    Result, frontend_origin,
    recovery::{
        clone::native::{digest, json as strict, source},
        fixture_control::{git_root, portable_path},
        inputs::{document, domain},
        runtime::business_evidence::{self, Authority, Evidence, Source},
    },
};

pub(crate) fn request(arguments: &Value) -> Result<Value> {
    strict::exact(
        arguments,
        &[
            "backend",
            "runner_root",
            "proof",
            "tests_receipt",
            "runtime_receipt",
            "target_plan",
            "authority",
        ],
        "恢复业务证明核验参数",
    )?;
    let backend = path(&arguments["backend"])?;
    let runner = path(&arguments["runner_root"])?;
    let window = Window::begin()?;
    let mut inputs = Inputs::new(&backend);
    let result = verify(arguments, &backend, &runner, &mut inputs, &window);
    window.finish(inputs.finish(result))
}

fn verify(
    arguments: &Value,
    backend: &Path,
    runner: &Path,
    inputs: &mut Inputs,
    window: &Window,
) -> Result<Value> {
    let authority = &arguments["authority"];
    strict::exact(
        authority,
        &[
            "format_version",
            "kind",
            "record",
            "backup_manifest",
            "tools",
        ],
        "恢复业务权威",
    )?;
    check(
        authority["format_version"].as_u64() == Some(2)
            && authority["kind"] == "restore-business-authority",
        "恢复业务权威版本或类型无效",
    )?;
    let record = &authority["record"];
    require_record(record)?;
    let tools = &authority["tools"];
    strict::exact(
        tools,
        &["runner", "verifier", "native_verifier"],
        "恢复业务权威工具",
    )?;
    let runner_sha = clean_source(runner, &tools["runner"], "恢复测试 runner")?;
    let verifier_sha = clean_source(backend, &tools["verifier"], "恢复证明协调后端")?;
    verify_tool(backend, &tools["native_verifier"], inputs)?;
    let proof = inputs.read(&path(&arguments["proof"])?)?;
    let tests = inputs.read(&path(&arguments["tests_receipt"])?)?;
    let verified = VerifiedTarget::read(
        backend,
        &path(&arguments["runtime_receipt"])?,
        &path(&arguments["target_plan"])?,
        inputs,
    )?;
    require_database_authority(
        &verified.manifest,
        &verified.documents.bindings.value["record"],
        authority,
    )?;
    let value = business_evidence::verify(
        Evidence {
            proof: &proof,
            tests: &tests,
            runtime: &verified.runtime,
            target: &verified.target,
        },
        Authority {
            record,
            runtime: &verified.authority,
            runner: Source {
                root: runner,
                sha: &runner_sha,
            },
            verifier: Source {
                root: backend,
                sha: &verifier_sha,
            },
            source_generation: &verified.lineage["source_generation"],
            dataset_lineage: &verified.lineage["dataset_lineage"],
        },
    )?;
    verified.verify_live(backend, window)?;
    Ok(value.clone())
}

fn require_database_authority(manifest: &Value, record: &Value, authority: &Value) -> Result<()> {
    check(
        *manifest == authority["backup_manifest"] && *record == authority["record"],
        "正式备份或运行记录与数据库权威不同",
    )
}

fn require_record(record: &Value) -> Result<()> {
    strict::exact(
        record,
        &[
            "plan",
            "plan_hash",
            "status",
            "started_at",
            "data_verified_at",
            "completed_at",
            "recovered_at",
            "failure",
        ],
        "恢复业务权威记录",
    )?;
    check(
        record["status"] == "data_verified"
            && record["completed_at"].is_null()
            && record["failure"].is_null(),
        "恢复业务权威记录尚未完成数据验证",
    )?;
    document::digest(&record["plan_hash"], "权威恢复计划摘要")?;
    for field in ["started_at", "data_verified_at", "recovered_at"] {
        domain::timestamp(&record[field], "权威恢复记录时间")?;
    }
    Ok(())
}

fn clean_source(root: &Path, expected: &Value, label: &str) -> Result<String> {
    strict::exact(expected, &["root", "sha"], label)?;
    check(
        path(&expected["root"])? == root && git_root(root, label)? == root,
        "恢复证明根与独立源码绑定不同",
    )?;
    let sha = domain::hash_40(&expected["sha"], label)?;
    let inventory = source::capture(root)?;
    check(
        inventory.source.snapshot.clean && inventory.source.snapshot.head == sha,
        "恢复证明要求精确 SHA 的干净工作树",
    )?;
    Ok(sha)
}

fn verify_tool(backend: &Path, expected: &Value, inputs: &mut Inputs) -> Result<()> {
    let binding = document::binding_shape(expected, "恢复证明原生核验程序")?;
    let executable = portable_path(&std::env::current_exe()?.canonicalize()?);
    check(
        binding.path == executable,
        "恢复证明核验程序与数据库权威绑定不同",
    )?;
    inputs.protect(&binding.path)?;
    let observed = digest::file(&binding.path)?;
    check(
        observed.bytes == binding.bytes && observed.sha256 == binding.sha256,
        "恢复证明核验程序字节与绑定不同",
    )?;
    let compiled = frontend_origin::describe()?;
    frontend_origin::verify_compiled_sources(backend, &compiled["sources"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formal_manifest_and_runtime_record_must_both_match_database_authority() {
        let manifest = json!({"backup_id":"backup-1","files":[{"sha256":"a".repeat(64)}]});
        let record = json!({"plan_hash":"b".repeat(64),"status":"data_verified",
            "data_verified_at":"2026-11-11T01:00:00Z"});
        let authority = json!({"backup_manifest":manifest,"record":record});
        require_database_authority(&manifest, &record, &authority).unwrap();

        let mut altered = manifest.clone();
        altered["files"][0]["sha256"] = json!("c".repeat(64));
        assert!(require_database_authority(&altered, &record, &authority).is_err());
        let mut altered = record.clone();
        altered["data_verified_at"] = json!("2026-11-11T01:01:00Z");
        assert!(require_database_authority(&manifest, &altered, &authority).is_err());
    }

    #[test]
    fn database_record_requires_data_verification_and_exact_fields() {
        let record = json!({
            "plan": {},
            "plan_hash": "a".repeat(64),
            "status": "data_verified",
            "started_at": "2026-11-11T00:55:00Z",
            "data_verified_at": "2026-11-11T01:00:00Z",
            "completed_at": null,
            "recovered_at": "2026-11-11T00:59:00Z",
            "failure": null,
        });
        require_record(&record).unwrap();

        for (field, value) in [
            ("status", json!("started")),
            ("completed_at", json!("2026-11-11T01:10:00Z")),
            ("failure", json!("failed")),
            ("data_verified_at", Value::Null),
            ("unexpected", json!(true)),
        ] {
            let mut changed = record.clone();
            changed[field] = value;
            assert!(require_record(&changed).is_err(), "{field}");
        }
    }
}

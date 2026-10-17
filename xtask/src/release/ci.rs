use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{ReleaseCiEvidenceOptions, ReleaseCiOperation, ReleaseCiOptions},
    workspace::root_dir,
};

use super::{
    archive::{BUSINESS_MODEL, source_pair_receipt, verify_source_pair_receipt},
    evidence::Requirement,
    github::{GhApi, redact_diagnostic},
    support::{canonical_dir, sha256_file, write_json, write_new_json},
};

#[path = "ci/artifacts.rs"]
pub(crate) mod artifacts;
#[path = "ci/collect.rs"]
pub(crate) mod collect;

pub(crate) fn run(options: &ReleaseCiOptions, frontend_dir: &Path) -> Result<()> {
    match &options.operation {
        ReleaseCiOperation::Evidence(options) => run_evidence(options),
        ReleaseCiOperation::RecordPair { output } => {
            run_pair(frontend_dir, output, PairOperation::Record)
        }
        ReleaseCiOperation::VerifyPair { input } => {
            run_pair(frontend_dir, input, PairOperation::Verify)
        }
    }
}

enum PairOperation {
    Record,
    Verify,
}

fn run_pair(frontend_dir: &Path, receipt: &Path, operation: PairOperation) -> Result<()> {
    let backend = canonical_dir(&root_dir(), "后端目录")?;
    let frontend = canonical_dir(frontend_dir, "前端目录")?;
    let (run_id, attempt) = github_run_identity()?;
    match operation {
        PairOperation::Record => {
            let value = source_pair_receipt(&backend, &frontend, run_id, attempt)?;
            write_new_json(receipt, &value)?;
            println!(
                "全栈源码组合已记录：run {run_id} / attempt {attempt}，{}",
                receipt.display()
            );
        }
        PairOperation::Verify => {
            verify_source_pair_receipt(receipt, &backend, &frontend, run_id, attempt)?;
            println!("全栈源码组合复核通过");
        }
    }
    Ok(())
}

fn run_evidence(options: &ReleaseCiEvidenceOptions) -> Result<()> {
    if let Some(parent) = options.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建发布证据目录 {}：{error}", parent.display()))?;
    }
    let timeout = Duration::from_secs(u64::from(options.timeout_seconds));
    let deadline = Instant::now() + timeout;
    let fixture = root_dir().join(BUSINESS_MODEL);
    let fixture_sha = sha256_file(&fixture, "业务 crate fixture 模型")?;
    let required = requirements(options);
    let tag_requirements = [
        required[0].clone(),
        Requirement {
            repository: options.frontend_repository.as_str().to_owned(),
            workflow: "ci.yml".to_owned(),
            sha: options.frontend_sha.as_str().to_owned(),
            jobs: Vec::new(),
            tag: Some(options.tag.as_str().to_owned()),
        },
    ];
    let tag_oids = options.tag_oids.as_ref().map(|oids| {
        (
            oids.backend.as_str().to_owned(),
            oids.frontend.as_str().to_owned(),
        )
    });
    let api = GhApi;
    let collected = collect::coordinated_evidence(
        deadline,
        &required,
        options.frontend_sha.as_str(),
        &fixture_sha,
        tag_oids.as_ref(),
        Some(&tag_requirements),
        &api,
    );
    let evidence = match collected {
        Ok(evidence) => evidence,
        Err(error) => {
            let message = redact_diagnostic(&error.to_string());
            let failure = failure_receipt(options, tag_oids.as_ref(), &message);
            if let Err(write_error) = write_json(&options.output, &failure) {
                return Err(format!("{message}；同时无法写入发布失败收据：{write_error}").into());
            }
            return Err(message.into());
        }
    };
    let mut success = match evidence {
        Value::Object(object) => object,
        _ => return Err("发布 CI 证据不是 JSON 对象".into()),
    };
    success.insert("status".to_owned(), json!("success"));
    let success = Value::Object(success);
    write_json(&options.output, &success)?;
    println!("双方日常 CI、Extended CI 和精确源码组合验证通过");
    Ok(())
}

pub(crate) fn requirements(options: &ReleaseCiEvidenceOptions) -> [Requirement; 2] {
    [
        Requirement {
            repository: options.backend_repository.as_str().to_owned(),
            workflow: "ci.yml".to_owned(),
            sha: options.backend_sha.as_str().to_owned(),
            jobs: [
                "Plan & Preflight",
                "Rust Gate",
                "Resource & Contract Gate",
                "MySQL 8.0.46, Redis 7.4.9 & AWS-LC TLS Integration",
                "Windows Smoke",
                "Security, Supply Chain & Deployment",
                "Required",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            tag: Some(options.tag.as_str().to_owned()),
        },
        Requirement {
            repository: options.backend_repository.as_str().to_owned(),
            workflow: "extended-ci.yml".to_owned(),
            sha: options.backend_sha.as_str().to_owned(),
            jobs: [
                "Real API MySQL Redis full-stack build",
                "Generated business crate data migration build",
                "Linux DevEx Cgroup Memory",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            tag: Some(options.tag.as_str().to_owned()),
        },
    ]
}

fn github_run_identity() -> Result<(u64, u64)> {
    Ok((
        environment_positive_integer("GITHUB_RUN_ID")?,
        environment_positive_integer("GITHUB_RUN_ATTEMPT")?,
    ))
}

fn environment_positive_integer(name: &str) -> Result<u64> {
    env::var(name)
        .map_err(|_| format!("缺少环境变量 {name}"))?
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("环境变量 {name} 必须是正整数").into())
}

pub(crate) fn failure_receipt(
    options: &ReleaseCiEvidenceOptions,
    tag_oids: Option<&(String, String)>,
    error: &str,
) -> Value {
    let mut failure = json!({
        "status": "failed",
        "error": error,
        "backend_sha": options.backend_sha.as_str(),
        "frontend_sha": options.frontend_sha.as_str(),
        "tag": options.tag.as_str(),
    });
    if let Some((backend, frontend)) = tag_oids {
        failure["backend_tag_oid"] = json!(backend);
        failure["frontend_tag_oid"] = json!(frontend);
    }
    failure
}

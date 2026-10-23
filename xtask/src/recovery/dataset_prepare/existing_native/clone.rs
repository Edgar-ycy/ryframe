use super::{
    super::{documents, existing, model},
    common,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::ffi::OsString;

const REQUIRED: &[&str] = &[
    "--backend-dir",
    "--plan",
    "--dataset",
    "--target-binding",
    "--configuration",
    "--run-dir",
    "--attempt",
];

pub(super) fn run(args: &[OsString]) -> Result<()> {
    let args = common::arguments(args, REQUIRED, "--write")?;
    let root = common::path(&args, "--backend-dir")?;
    if root != std::env::current_dir()? {
        return Err("复制已有数据后端目录必须是当前工作目录".into());
    }
    let run = common::path(&args, "--run-dir")?;
    validate_local_test_path(&run, &root, LocalTestPathKind::ExistingDirectory)?;
    let attempt = args["--attempt"]
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or("复制已有数据 attempt 必须为正整数")?;
    let (plan_snapshot, plan) = common::Snapshot::read(
        &root,
        &common::path(&args, "--plan")?,
        16 * 1024 * 1024,
        "原参考计划",
    )?;
    let (dataset_snapshot, dataset) = common::Snapshot::read(
        &root,
        &common::path(&args, "--dataset")?,
        16 * 1024 * 1024,
        "原数据集",
    )?;
    let (target_snapshot, target) = common::Snapshot::read(
        &root,
        &common::path(&args, "--target-binding")?,
        1024 * 1024,
        "复制目标绑定",
    )?;
    model::specification(&plan)?;
    model::validate_dataset(&plan, &dataset)?;
    validate_target(&plan, &target)?;
    let contract_path = root.join("openapi/openapi.json");
    let (contract_snapshot, contract) =
        common::Snapshot::read(&root, &contract_path, 16 * 1024 * 1024, "OpenAPI契约")?;
    let catalog = common::catalog(&contract)?;
    let mut snapshots = vec![
        plan_snapshot,
        dataset_snapshot,
        target_snapshot,
        contract_snapshot,
    ];
    let mut configuration_snapshots = Vec::new();
    let password = common::password(
        &root,
        &root,
        &common::path(&args, "--configuration")?,
        &plan["dataset"],
        &mut snapshots,
        &mut configuration_snapshots,
    )?;
    common::wait_start(&json!({"operation":"start","run_dir":run,"attempt":attempt}))?;
    let counts = runtime(&plan, &dataset, &target, &catalog, &password, || {
        common::unchanged(&root, &snapshots, &configuration_snapshots)
    })?;
    common::unchanged(&root, &snapshots, &configuration_snapshots)?;
    common::write_result(&json!({
        "format_version":1,
        "status":"copy_existing_data_verified",
        "side":"copy_target",
        "scope_id":target["target"]["scope_id"],
        "plan_sha256":model::hash(&plan)?,
        "source_scope_id":dataset["source_scope_id"],
        "target_binding_sha256":model::hash(&target)?,
        "copy":target["copy"],
        "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
        "clone_verified":false,
        "restore_success":false,
        "tenants":counts.tenants,
        "posts":counts.posts,
        "files":counts.files,
        "input_files":{
            "plan_sha256":snapshots[0].sha256(),
            "dataset_sha256":snapshots[1].sha256(),
            "target_binding_sha256":snapshots[2].sha256(),
        },
    }))
}

fn runtime(
    plan: &Value,
    dataset: &Value,
    target: &Value,
    catalog: &std::collections::BTreeMap<String, Value>,
    password: &str,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<existing::Counts> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(existing::verify_at(
        &target["target"],
        dataset["tenants"].as_array().ok_or("原数据集租户缺失")?,
        catalog,
        password,
        existing::Options {
            network: 19,
            request_interval_ms: plan["dataset"]["request_interval_ms"]
                .as_u64()
                .ok_or("请求节奏缺失")?,
            collect_subjects: false,
        },
        checkpoint,
    ))
}

pub(super) fn validate_target(plan: &Value, value: &Value) -> Result<()> {
    documents::exact(
        value,
        &[
            "format_version",
            "kind",
            "source_plan_sha256",
            "source_scope_id",
            "target",
            "copy",
        ],
        "复制目标绑定",
    )?;
    documents::exact(
        &value["target"],
        &["scope_id", "api_url", "frontend_url"],
        "复制目标",
    )?;
    documents::exact(
        &value["copy"],
        &[
            "plan_sha256",
            "generation_sha256",
            "source_export_sha256",
            "fresh_target_sha256",
            "stage_receipt_sha256",
            "ledger_head_sha256",
        ],
        "复制证据",
    )?;
    let scope = value["target"]["scope_id"]
        .as_str()
        .ok_or("复制目标scope缺失")?;
    let valid_scope = regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?;
    let valid_sha = regex::Regex::new(r"^[a-f0-9]{64}$")?;
    if value["format_version"] != 1
        || value["kind"] != "devex-copy-business-target"
        || value["source_plan_sha256"] != model::hash(plan)?
        || value["source_scope_id"] != plan["source"]["scope_id"]
        || !valid_scope.is_match(scope)
        || value["target"]["scope_id"] == plan["source"]["scope_id"]
        || value["copy"].as_object().is_none_or(|fields| {
            fields
                .values()
                .any(|item| item.as_str().is_none_or(|sha| !valid_sha.is_match(sha)))
        })
    {
        return Err("复制目标没有绑定原数据计划和明确复制成功证据".into());
    }
    model::endpoint(&value["target"]["api_url"])?;
    model::endpoint(&value["target"]["frontend_url"])?;
    Ok(())
}

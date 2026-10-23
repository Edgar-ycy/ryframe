use super::{
    super::{documents, existing, model},
    common,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, ffi::OsString};

const REQUIRED: &[&str] = &[
    "--backend-dir",
    "--lineage",
    "--contract",
    "--staging-manifest",
    "--configuration",
    "--configuration-root",
    "--run-dir",
    "--operation-id",
    "--source-generation-sha256",
];
pub(super) const DESCRIPTORS: &[&str] = &[
    "source_registration",
    "seed_registration",
    "post_copy",
    "post_verify",
    "post_verify_evidence",
    "post_verify_target",
    "reference_plan",
    "dataset",
    "copy_stage_receipt",
    "copy_result",
    "ledger_head",
    "copy_plan",
    "current_image",
];

pub(super) fn run(args: &[OsString]) -> Result<()> {
    let args = common::arguments(args, REQUIRED, "--write")?;
    let root = common::path(&args, "--backend-dir")?;
    if root != std::env::current_dir()? {
        return Err("来源已有数据后端目录必须是当前工作目录".into());
    }
    let run = common::path(&args, "--run-dir")?;
    validate_local_test_path(&run, &root, LocalTestPathKind::ExistingDirectory)?;
    if run.file_name().and_then(|value| value.to_str()) != Some("verification") {
        return Err("来源已有数据必须绑定verification目录".into());
    }
    let operation = args["--operation-id"].as_str();
    let generation = args["--source-generation-sha256"].as_str();
    if !digest(operation, 32) || !digest(generation, 64) {
        return Err("来源已有数据运行身份摘要无效".into());
    }
    let lineage_path = common::path(&args, "--lineage")?;
    if lineage_path
        != run
            .parent()
            .ok_or("verification目录缺少代次父目录")?
            .join("dataset-lineage.json")
    {
        return Err("来源已有数据血缘不属于同代verification目录".into());
    }
    let (lineage_snapshot, lineage) =
        common::Snapshot::read(&root, &lineage_path, 16 * 1024 * 1024, "派生数据血缘")?;
    validate_lineage(&lineage)?;
    let contract_path = common::path(&args, "--contract")?;
    let (contract_snapshot, contract) =
        common::Snapshot::read(&root, &contract_path, 16 * 1024 * 1024, "来源OpenAPI契约")?;
    let manifest_path = common::path(&args, "--staging-manifest")?;
    let (manifest_snapshot, manifest) =
        common::Snapshot::read(&root, &manifest_path, 1024 * 1024, "来源原生staging清单")?;
    let runtime_files_sha256 = validate_staging(&manifest, &contract_path, &contract_snapshot)?;
    let catalog = common::catalog(&contract)?;
    if catalog
        .get("get_system_posts_by_id")
        .is_none_or(|value| value["method"] != "GET")
    {
        return Err("旧岗位验收必须使用只读查询契约".into());
    }
    let mut snapshots = vec![lineage_snapshot, contract_snapshot, manifest_snapshot];
    let mut configuration_snapshots = Vec::new();
    let password = common::password(
        &root,
        &common::path(&args, "--configuration-root")?,
        &common::path(&args, "--configuration")?,
        &mut snapshots,
        &mut configuration_snapshots,
    )?;
    let ready = json!({
        "format_version":1,
        "kind":"restore-source-producer-ready",
        "operation_id":operation,
        "source_generation_sha256":generation,
        "staging_manifest_sha256":snapshots[2].sha256(),
        "runtime_files_sha256":runtime_files_sha256,
        "contract_file_sha256":snapshots[1].sha256(),
        "lineage_file_sha256":snapshots[0].sha256(),
    });
    common::write_result(&ready)?;
    common::wait_start(&json!({
        "operation":"start",
        "run_dir":run,
        "operation_id":operation,
        "source_generation_sha256":generation,
    }))?;
    let counts = runtime(&lineage, &catalog, &password, || {
        common::unchanged(&root, &snapshots, &configuration_snapshots)
    })?;
    common::unchanged(&root, &snapshots, &configuration_snapshots)?;
    common::write_result(&json!({
        "format_version":1,
        "kind":"restore-source-existing-verification",
        "status":"source_existing_data_verified",
        "scope_id":lineage["verification"]["scope_id"],
        "origin_tenant_scope_id":lineage["scopes"]["origin_tenant_scope_id"],
        "lineage_sha256":model::hash(&lineage)?,
        "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
        "restore_success":false,
        "subjects":counts.subjects,
        "tenants":counts.tenants,
        "posts":counts.posts,
        "files":counts.files,
        "source_generation_sha256":generation,
        "lineage_file_sha256":snapshots[0].sha256(),
    }))
}

fn runtime(
    lineage: &Value,
    catalog: &std::collections::BTreeMap<String, Value>,
    password: &str,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<existing::Counts> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(existing::verify_at(
        &lineage["verification"],
        lineage["tenants"].as_array().ok_or("派生来源租户缺失")?,
        catalog,
        password,
        existing::Options {
            network: 18,
            request_interval_ms: lineage["verification"]["request_interval_ms"]
                .as_u64()
                .ok_or("请求节奏缺失")?,
            collect_subjects: true,
        },
        checkpoint,
    ))
}

fn validate_staging(
    manifest: &Value,
    contract_path: &std::path::Path,
    contract: &common::Snapshot,
) -> Result<String> {
    documents::exact(
        manifest,
        &[
            "format_version",
            "kind",
            "coordinator_source",
            "execution_sha",
            "entry",
            "contract",
            "runtime_modules",
            "files",
        ],
        "来源原生staging清单",
    )?;
    let modules = manifest["runtime_modules"]
        .as_array()
        .filter(|value| value.len() == 1)
        .ok_or("来源原生staging必须绑定唯一Rust工具")?;
    documents::exact(&modules[0], &["path", "bytes", "sha256"], "来源原生工具")?;
    let files = manifest["files"]
        .as_array()
        .filter(|value| value.len() == 1)
        .ok_or("来源原生staging必须绑定唯一OpenAPI文件")?;
    documents::exact(
        &files[0],
        &["path", "origin", "bytes", "sha256"],
        "来源契约文件",
    )?;
    if manifest["format_version"] != 2
        || manifest["kind"] != "restore-source-native-staging"
        || manifest["entry"] != super::super::existing_native::SOURCE_MARKER
        || manifest["contract"] != contract_path.to_string_lossy().as_ref()
        || files[0]["path"] != contract_path.to_string_lossy().as_ref()
        || files[0]["origin"] != "execution"
        || files[0]["bytes"].as_u64().is_none()
        || files[0]["sha256"] != contract.sha256()
        || !digest(modules[0]["sha256"].as_str().unwrap_or(""), 64)
        || modules[0]["bytes"].as_u64().is_none_or(|value| value == 0)
    {
        return Err("来源原生staging没有绑定Rust工具或OpenAPI契约".into());
    }
    model::hash(&Value::Array(modules.clone()))
}

pub(in crate::recovery) fn validate_lineage(value: &Value) -> Result<()> {
    let mut fields = vec!["format_version", "kind", "status"];
    fields.extend_from_slice(DESCRIPTORS);
    fields.extend_from_slice(&[
        "scopes",
        "scale",
        "tenants",
        "objects",
        "verification",
        "restore_qualified",
    ]);
    documents::exact(value, &fields, "派生数据血缘")?;
    for field in DESCRIPTORS {
        documents::exact(
            &value[*field],
            &["bytes", "path", "sha256"],
            "派生血缘文件绑定",
        )?;
        if value[*field]["bytes"].as_u64().is_none_or(|size| size == 0)
            || value[*field]["path"]
                .as_str()
                .is_none_or(|path| !std::path::Path::new(path).is_absolute())
            || !digest(value[*field]["sha256"].as_str().unwrap_or(""), 64)
        {
            return Err("派生血缘文件绑定无效".into());
        }
    }
    documents::exact(
        &value["scopes"],
        &[
            "origin_tenant_scope_id",
            "current_source_scope_id",
            "current_object_scope_id",
        ],
        "派生血缘scope",
    )?;
    documents::exact(
        &value["scale"],
        &[
            "records",
            "current_post_rows",
            "tenants",
            "post_samples",
            "business_objects",
            "verified_objects",
            "object_bytes",
        ],
        "派生血缘规模",
    )?;
    documents::exact(
        &value["verification"],
        &["scope_id", "api_url", "frontend_url", "request_interval_ms"],
        "派生来源运行参数",
    )?;
    documents::exact(
        &value["objects"],
        &[
            "business_objects",
            "business_bytes",
            "mapping_sha256",
            "probe",
            "verified_objects",
        ],
        "派生对象投影",
    )?;
    if value["format_version"] != 1
        || value["kind"] != "restore-source-derived-dataset-lineage"
        || value["status"] != "derived_dataset_verified"
        || value["restore_qualified"] != false
        || value["scopes"]["current_source_scope_id"] != value["verification"]["scope_id"]
        || value["scopes"]["current_object_scope_id"] != value["verification"]["scope_id"]
        || value["scopes"]["origin_tenant_scope_id"] == value["verification"]["scope_id"]
        || model::integer(&value["scale"]["records"], 100_000, u64::MAX, "派生记录数").is_err()
        || value["scale"]["current_post_rows"].as_u64() < value["scale"]["records"].as_u64()
        || value["scale"]["tenants"] != 11
        || value["scale"]["business_objects"] != 256
        || value["scale"]["verified_objects"] != 257
        || value["scale"]["object_bytes"]
            .as_u64()
            .is_none_or(|bytes| bytes < 1024 * 1024 * 1024)
    {
        return Err("派生数据血缘没有绑定正式恢复规模或当前来源".into());
    }
    model::endpoint(&value["verification"]["api_url"])?;
    model::endpoint(&value["verification"]["frontend_url"])?;
    model::integer(
        &value["verification"]["request_interval_ms"],
        1000,
        5000,
        "请求节奏",
    )?;
    validate_probe(value)?;
    validate_tenants(value)
}

fn validate_probe(value: &Value) -> Result<()> {
    let probe = &value["objects"]["probe"];
    documents::exact(
        probe,
        &[
            "bucket",
            "source_key",
            "target_key",
            "bytes",
            "sha256",
            "metadata",
        ],
        "派生来源唯一探针",
    )?;
    let origin = format!(
        "{}/",
        value["scopes"]["origin_tenant_scope_id"]
            .as_str()
            .unwrap_or("")
    );
    let current = format!(
        "{}/",
        value["scopes"]["current_object_scope_id"]
            .as_str()
            .unwrap_or("")
    );
    if !["uploads", "avatar", "exports", "imports", "config-packages"]
        .contains(&probe["bucket"].as_str().unwrap_or(""))
        || probe["source_key"]
            .as_str()
            .is_none_or(|key| !key.starts_with(&origin))
        || probe["target_key"]
            .as_str()
            .is_none_or(|key| !key.starts_with(&current))
        || model::integer(&probe["bytes"], 1, 4 * 1024 * 1024, "探针对象大小").is_err()
        || !digest(probe["sha256"].as_str().unwrap_or(""), 64)
        || !probe["metadata"].is_object()
    {
        return Err("派生来源唯一探针没有绑定原始及当前对象scope".into());
    }
    Ok(())
}

fn validate_tenants(value: &Value) -> Result<()> {
    let tenants = value["tenants"]
        .as_array()
        .filter(|items| items.len() == 11)
        .ok_or("派生来源必须包含十一租户")?;
    let origin = value["scopes"]["origin_tenant_scope_id"]
        .as_str()
        .ok_or("原租户scope缺失")?;
    let mut ids = BTreeSet::new();
    let mut totals = (0u64, 0u64, 0u64, 0u64);
    for (index, identity) in tenants.iter().enumerate() {
        documents::exact(
            identity,
            &[
                "tenant_id",
                "database",
                "username",
                "records",
                "posts",
                "files",
            ],
            "派生来源租户",
        )?;
        let expected = if index == 0 {
            "system".into()
        } else {
            format!("{origin}-{index:02}")
        };
        let id = identity["tenant_id"].as_str().ok_or("派生租户ID缺失")?;
        let posts = identity["posts"]
            .as_array()
            .filter(|items| items.len() >= 3)
            .ok_or("派生岗位样本不足")?;
        let files = identity["files"]
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or("派生对象样本缺失")?;
        if id != expected
            || !ids.insert(id)
            || identity["username"].as_str().is_none_or(str::is_empty)
        {
            return Err("派生来源租户身份或归属无效".into());
        }
        for post in posts {
            documents::exact(post, &["id", "code", "name"], "派生岗位样本")?;
            model::positive_id(&post["id"])?;
        }
        for file in files {
            documents::exact(
                file,
                &[
                    "file_id",
                    "file_name",
                    "file_path",
                    "file_url",
                    "bytes",
                    "sha256",
                ],
                "派生对象样本",
            )?;
            model::positive_id(&file["file_id"])?;
            if file["file_path"]
                .as_str()
                .is_none_or(|path| !path.starts_with(&format!("{id}/")))
                || model::integer(&file["bytes"], 1, 4 * 1024 * 1024, "派生对象大小").is_err()
                || !digest(file["sha256"].as_str().unwrap_or(""), 64)
            {
                return Err("派生来源对象样本无效".into());
            }
            totals.3 = totals
                .3
                .checked_add(file["bytes"].as_u64().unwrap_or(0))
                .ok_or("派生对象字节总数溢出")?;
        }
        totals.0 = totals
            .0
            .checked_add(identity["records"].as_u64().ok_or("派生租户记录数无效")?)
            .ok_or("派生租户记录总数溢出")?;
        totals.1 += posts.len() as u64;
        totals.2 += files.len() as u64;
    }
    if totals
        != (
            value["scale"]["records"].as_u64().unwrap_or(0),
            value["scale"]["post_samples"].as_u64().unwrap_or(0),
            value["scale"]["business_objects"].as_u64().unwrap_or(0),
            value["scale"]["object_bytes"].as_u64().unwrap_or(0),
        )
    {
        return Err("派生来源租户汇总与完整血缘规模不同".into());
    }
    Ok(())
}

fn digest(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

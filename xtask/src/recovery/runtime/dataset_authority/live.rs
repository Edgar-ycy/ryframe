//! 在恢复运行代次内核验登记的既有租户数据并注销会话。
use std::path::Path;

use serde_json::{Value, json};

use super::{inputs::Inputs, path};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{json as strict, s3::app_configuration},
        dataset_prepare::{configuration, existing, existing_native::source::validate_lineage},
        fixture_control::services::lifecycle::readonly::process,
        inputs::domain,
    },
};

pub(super) fn verify(
    backend: &Path,
    inputs: &mut Inputs,
    authority: &Value,
    runtime: &Value,
    reference: &Value,
    lineage: &Value,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Value> {
    validate_lineage(lineage)?;
    let target = &authority["target"];
    let api = registered_api(target, runtime)?;
    let identity = &runtime["processes"]["api"]["identity"];
    let contract =
        inputs.read(&path(&authority["execution_backend"])?.join("openapi/openapi.json"))?;
    let catalog = crate::devex::runtime_configuration::operation_catalog(&contract.value)?;
    let dataset = &reference["dataset"];
    let credential = path(&dataset["administrator_credentials"]["path"])?;
    validate_local_test_path(&credential, backend, LocalTestPathKind::ExistingFile)?;
    inputs.protect(&credential)?;
    let mut snapshots = Vec::new();
    let password = configuration::current(backend, dataset, &mut snapshots)?;
    let tenants = strict::array(&lineage["tenants"], "派生数据租户")?;
    let request_interval_ms = lineage["verification"]["request_interval_ms"]
        .as_u64()
        .ok_or("派生数据请求间隔缺失")?;
    let mut check_live = || {
        checkpoint()?;
        app_configuration::unchanged(&snapshots)?;
        process::listener(identity, api)
    };
    check_live()?;
    crate::http_session::install_crypto_provider()?;
    let verified = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(existing::verify_at(
            target,
            tenants,
            &catalog,
            &password,
            existing::Options {
                network: 19,
                request_interval_ms,
                collect_subjects: false,
            },
            &mut check_live,
        ));
    let after = check_live();
    let counts = verified?;
    after?;
    receipt(target, lineage, counts)
}

fn registered_api<'a>(target: &Value, runtime: &'a Value) -> Result<&'a str> {
    let ready = &runtime["endpoints"]["api"];
    let endpoint = domain::endpoint(ready, "恢复运行 API 就绪端点", Some("/readyz"))?;
    let api = strict::text(ready, "恢复运行 API 就绪端点")?
        .strip_suffix("/readyz")
        .ok_or("恢复运行 API 就绪路径无效")?;
    let target_api = strict::text(&target["api_url"], "恢复目标 API 端点")?;
    if endpoint.scheme != "http"
        || endpoint.host != "127.0.0.1"
        || target_api.strip_suffix('/').unwrap_or(target_api) != api
        || target["frontend_url"] != runtime["endpoints"]["frontend"]
        || target["scope_id"] != runtime["restore"]["scope_id"]
    {
        return Err("恢复目标和登记运行的 scope、前端或 API 端点不同".into());
    }
    Ok(api)
}

fn receipt(target: &Value, lineage: &Value, counts: existing::Counts) -> Result<Value> {
    let scale = &lineage["scale"];
    if scale["tenants"].as_u64() != Some(counts.tenants)
        || scale["post_samples"].as_u64() != Some(counts.posts)
        || scale["business_objects"].as_u64() != Some(counts.files)
    {
        return Err("恢复目标的租户、岗位样本或对象数量与登记血缘不同".into());
    }
    Ok(json!({
        "format_version": 1,
        "kind": "restore-target-existing-verification",
        "status": "target_existing_data_verified",
        "scope_id": target["scope_id"],
        "source_scope_id": lineage["scopes"]["origin_tenant_scope_id"],
        "actions": {"business": "read_only", "objects": "read_only", "session": "login_logout"},
        "restore_success": false,
        "tenants": counts.tenants,
        "posts": counts.posts,
        "files": counts.files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_api_must_match_the_registered_readiness_listener() {
        let runtime = json!({
            "restore": {"scope_id": "target-scope"},
            "endpoints": {
                "api": "http://127.0.0.1:23110/readyz",
                "frontend": "http://127.0.0.1:23111"
            },
        });
        let mut target = json!({
            "scope_id": "target-scope",
            "api_url": "http://127.0.0.1:23110/",
            "frontend_url": "http://127.0.0.1:23111"
        });
        assert_eq!(
            registered_api(&target, &runtime).unwrap(),
            "http://127.0.0.1:23110"
        );
        target["api_url"] = json!("http://127.0.0.1:23112");
        assert!(registered_api(&target, &runtime).is_err());
    }
}

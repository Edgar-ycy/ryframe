use super::*;

pub(super) fn bundle_model(
    tenant_id: &str,
    user_id: i64,
    file_id: i64,
    parsed: &ParsedTenantConfigPackage,
    now: DateTime<Utc>,
    artifact_hours: u32,
) -> AppResult<TenantConfigBundleRecord> {
    let bundle_id = next_id()?;
    let counts =
        serde_json::to_value(&parsed.manifest.resource_counts).map_err(internal_json_error)?;
    Ok(TenantConfigBundleRecord {
        id: bundle_id,
        tenant_id: tenant_id.to_owned(),
        origin: TenantConfigBundleRecord::ORIGIN_UPLOADED.to_owned(),
        source_tenant_key: parsed.manifest.source_tenant_key.clone(),
        source_tenant_name_snapshot: parsed.manifest.source_tenant_name.clone(),
        package_schema_version: parsed.manifest.schema.clone(),
        source_app_version: parsed.manifest.source_app_version.clone(),
        file_id: Some(file_id),
        sha256: Some(parsed.package_sha256.clone()),
        resource_counts: counts,
        item_count: i32::try_from(parsed.manifest.item_count)
            .map_err(|_| AppError::PayloadTooLarge("配置包项目数量超限".into()))?,
        status: TenantConfigBundleRecord::STATUS_SUCCEEDED.to_owned(),
        background_job_id: None,
        idempotency_key_hash: None,
        created_by: user_id,
        error_summary: None,
        expires_at: Some(now + Duration::hours(i64::from(artifact_hours))),
        created_at: now,
        updated_at: now,
    })
}

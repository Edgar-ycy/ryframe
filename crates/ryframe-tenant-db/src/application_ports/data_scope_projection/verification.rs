use super::*;
use ryframe_application::ports::tenant_data::TenantDataFence;

pub(super) async fn publish_verified(
    control: &DatabaseTransaction,
    target: &DatabaseTransaction,
    fence: TenantDataFence<'_>,
    version: i64,
) -> AppResult<()> {
    let metadata = load_projection(target, fence.tenant_id).await?;
    let current = metadata
        .as_ref()
        .map(|row| reusable_current(row, fence, version))
        .transpose()?;
    if current != Some(true) {
        write_projection(
            control,
            target,
            fence.tenant_id,
            version,
            &ProjectionPlacement {
                target_key: fence.target_key.into(),
                generation: fence.generation,
                switch_token: fence.switch_token.into(),
            },
        )
        .await?;
    }
    verify_publication(control, target, fence, version).await
}

fn reusable_current(
    metadata: &ProjectionRow,
    fence: TenantDataFence<'_>,
    version: i64,
) -> AppResult<bool> {
    let expected_token = build_token(
        fence.tenant_id,
        metadata.source_version,
        fence.target_key,
        metadata.placement_generation,
        &metadata.switch_token,
    );
    if metadata.source_version <= 0
        || metadata.source_version > version
        || metadata.placement_generation <= 0
        || metadata.placement_generation > fence.generation
        || metadata.switch_token.trim().is_empty()
        || metadata.build_token != expected_token
        || !valid_digest(&metadata.digest)
        || !metadata.ready
        || metadata.owner_count < 0
        || metadata.dept_count < 0
        || (metadata.placement_generation == fence.generation
            && metadata.switch_token != fence.switch_token)
    {
        return Err(AppError::Conflict(
            "迁移目标投影版本或已发布身份发生漂移".into(),
        ));
    }
    Ok(metadata.source_version == version && metadata.placement_generation == fence.generation)
}

async fn verify_publication(
    control: &DatabaseTransaction,
    target: &DatabaseTransaction,
    fence: TenantDataFence<'_>,
    version: i64,
) -> AppResult<()> {
    let metadata = load_projection(target, fence.tenant_id)
        .await?
        .ok_or_else(|| AppError::Conflict("迁移目标缺少已发布投影".into()))?;
    let expected = rows::scan_projection(control, fence.tenant_id, None, None).await?;
    let actual =
        rows::scan_projection(target, fence.tenant_id, Some(&metadata.build_token), None).await?;
    if !reusable_current(&metadata, fence, version)?
        || !metadata.ready
        || (
            metadata.owner_count,
            metadata.dept_count,
            metadata.digest.clone(),
        ) != expected
        || actual != expected
        || !projection_rows_match(target, fence.tenant_id, &metadata).await?
    {
        return Err(AppError::Conflict(
            "迁移目标投影元数据或实际身份、部门行与控制快照不一致".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence() -> TenantDataFence<'static> {
        TenantDataFence {
            tenant_id: "tenant",
            target_key: "target",
            generation: 3,
            switch_token: "new",
        }
    }

    fn metadata(version: i64, generation: i64, switch_token: &str) -> ProjectionRow {
        ProjectionRow {
            source_version: version,
            build_token: build_token("tenant", version, "target", generation, switch_token),
            placement_generation: generation,
            switch_token: switch_token.into(),
            digest: "a".repeat(64),
            owner_count: 1,
            dept_count: 1,
            ready: true,
        }
    }

    #[test]
    fn migration_rebuilds_old_generations_and_rejects_future_or_changed_identity() {
        assert!(!reusable_current(&metadata(7, 2, "old"), fence(), 8).unwrap());
        assert!(!reusable_current(&metadata(8, 2, "old"), fence(), 8).unwrap());
        assert!(!reusable_current(&metadata(7, 3, "new"), fence(), 8).unwrap());
        assert!(reusable_current(&metadata(8, 3, "new"), fence(), 8).unwrap());
        for row in [
            metadata(9, 2, "old"),
            metadata(8, 4, "future"),
            metadata(8, 3, "wrong"),
            metadata(8, 0, "invalid"),
        ] {
            assert!(matches!(
                reusable_current(&row, fence(), 8),
                Err(AppError::Conflict(_))
            ));
        }
        let mut invalid = metadata(7, 2, "old");
        invalid.build_token = "0".repeat(64);
        assert!(reusable_current(&invalid, fence(), 8).is_err());
        let mut unpublished = metadata(7, 2, "old");
        unpublished.ready = false;
        assert!(reusable_current(&unpublished, fence(), 8).is_err());
    }
}

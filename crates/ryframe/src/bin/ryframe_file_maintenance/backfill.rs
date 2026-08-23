use super::*;

pub(super) async fn backfill_sha256(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    arguments: &Arguments,
) -> Result<(), DynError> {
    let mut cursor = arguments.start_after;
    let mut stats = BackfillStats::default();

    loop {
        let rows = LegacyDigestRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, bucket, storage_path, file_size, file_md5 \
             FROM sys_file WHERE id > ? AND file_sha256 IS NULL \
             ORDER BY id ASC LIMIT ?",
            [cursor.into(), i64::try_from(arguments.batch_size)?.into()],
        ))
        .all(database)
        .await?;
        if rows.is_empty() {
            break;
        }

        for row in rows {
            cursor = row.id;
            stats.scanned += 1;
            let legacy_md5 = row
                .file_md5
                .as_deref()
                .ok_or_else(|| format!("文件 {} 缺少旧 MD5，拒绝猜测摘要", row.id))?;
            let normalized_md5 = normalize_legacy_md5(legacy_md5)
                .map_err(|error| format!("文件 {} 的旧 MD5 无效: {error}", row.id))?;
            let object = storage
                .get(&row.bucket, &row.storage_path)
                .await
                .map_err(|error| format!("读取文件 {} 对象失败: {error}", row.id))?;
            let digests = tokio::task::spawn_blocking(move || calculate_object_digests(&object))
                .await
                .map_err(|error| format!("文件 {} 摘要任务失败: {error}", row.id))?;
            validate_backfill_object(row.id, row.file_size, &normalized_md5, &digests)?;
            let sha256 = digests.sha256;

            println!(
                "{} file_id={} cursor={} sha256={}",
                if arguments.mode == Mode::Apply {
                    "apply"
                } else {
                    "dry-run"
                },
                row.id,
                cursor,
                sha256
            );
            if arguments.mode == Mode::DryRun {
                continue;
            }

            let result = database
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    "UPDATE sys_file SET file_sha256 = ?, updated_at = UTC_TIMESTAMP(6) \
                     WHERE id = ? AND file_sha256 IS NULL AND file_md5 = ? \
                     AND bucket = ? AND storage_path = ? AND file_size = ?",
                    [
                        sha256.clone().into(),
                        row.id.into(),
                        legacy_md5.to_owned().into(),
                        row.bucket.clone().into(),
                        row.storage_path.clone().into(),
                        row.file_size.into(),
                    ],
                ))
                .await?;
            if result.rows_affected() == 1 {
                stats.updated += 1;
                continue;
            }

            let current = sys_file::Entity::find_by_id(row.id).one(database).await?;
            match classify_backfill_cas(
                current.as_ref().map(|file| file.file_sha256.as_str()),
                &sha256,
            ) {
                BackfillCasDecision::AlreadyApplied => stats.already_updated += 1,
                BackfillCasDecision::Conflict => {
                    return Err(
                        format!("文件 {} 的 SHA-256 CAS 失败，数据已发生变化", row.id).into(),
                    );
                }
            }
        }
    }

    let remaining = sys_file::Entity::find()
        .filter(sys_file::Column::FileSha256.is_null())
        .count(database)
        .await?;
    println!(
        "SHA-256 backfill summary: scanned={} updated={} already_updated={} remaining={}",
        stats.scanned, stats.updated, stats.already_updated, remaining
    );
    if arguments.mode == Mode::Apply && remaining != 0 {
        return Err(format!(
            "仍有 {remaining} 条文件记录缺少 SHA-256；请从更早游标重试并排除数据错误"
        )
        .into());
    }
    Ok(())
}

/// 一次遍历同时计算旧 MD5 校验值和新的 SHA-256 权威摘要。
fn calculate_object_digests(object: &[u8]) -> ObjectDigests {
    ObjectDigests {
        byte_len: object.len(),
        legacy_md5: format!("{:x}", md5::compute(object)),
        sha256: hex::encode(Sha256::digest(object)),
    }
}

/// 在写入 SHA-256 前校验旧元数据确实对应当前对象。
fn validate_backfill_object(
    file_id: i64,
    expected_size: i64,
    normalized_md5: &str,
    digests: &ObjectDigests,
) -> Result<(), DynError> {
    let expected_size =
        usize::try_from(expected_size).map_err(|_| format!("文件 {file_id} 的 file_size 非法"))?;
    if digests.byte_len != expected_size {
        return Err(format!(
            "文件 {file_id} 大小不一致（数据库: {expected_size}，对象: {}）",
            digests.byte_len
        )
        .into());
    }
    if digests.legacy_md5 != normalized_md5 {
        return Err(format!("文件 {file_id} 的旧 MD5 校验失败，拒绝写入 SHA-256").into());
    }
    Ok(())
}

/// CAS 未更新行时，只把已经写入相同 SHA-256 视为幂等成功。
fn classify_backfill_cas(
    current_sha256: Option<&str>,
    calculated_sha256: &str,
) -> BackfillCasDecision {
    if current_sha256 == Some(calculated_sha256) {
        BackfillCasDecision::AlreadyApplied
    } else {
        BackfillCasDecision::Conflict
    }
}

fn normalize_legacy_md5(value: &str) -> Result<String, &'static str> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("必须是 32 位十六进制字符串");
    }
    Ok(value.to_ascii_lowercase())
}

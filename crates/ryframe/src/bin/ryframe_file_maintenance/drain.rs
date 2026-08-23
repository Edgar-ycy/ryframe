use super::*;

pub(super) async fn drain_legacy_reservations(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    arguments: &Arguments,
) -> Result<(), DynError> {
    let now = database_utc_now(database).await?;
    if arguments.mode == Mode::Apply {
        let active_pending = sys_file::Entity::find()
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(
                Condition::any()
                    .add(sys_file::Column::ReservationExpiresAt.is_null())
                    .add(sys_file::Column::ReservationExpiresAt.gt(now)),
            )
            .count(database)
            .await?;
        if active_pending != 0 {
            return Err(format!(
                "检测到 {active_pending} 条仍有效或无到期时间的旧上传预留；请停止旧版 API/Worker，等待租约到期后重试"
            )
            .into());
        }
    }

    let cleanup_grace = cleanup_grace(storage);
    let mut cursor = arguments.start_after;
    let mut stats = DrainStats::default();
    loop {
        let rows = sys_file::Entity::find()
            .filter(sys_file::Column::Id.gt(cursor))
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .order_by_asc(sys_file::Column::Id)
            .limit(arguments.batch_size)
            .all(database)
            .await?;
        if rows.is_empty() {
            break;
        }

        for row in rows {
            cursor = row.id;
            stats.scanned += 1;
            match plan_legacy_reservation(&row, now)? {
                DrainPlan::NormalizeReady => {
                    println!(
                        "{:?} ready file_id={} -> del_flag=0",
                        arguments.mode, row.id
                    );
                    if arguments.mode == Mode::Apply
                        && normalize_ready_reservation(database, &row).await?
                    {
                        stats.normalized_ready += 1;
                    }
                }
                DrainPlan::MovePendingToCleanup => {
                    let cleanup_after = now + cleanup_grace;
                    println!(
                        "{:?} pending file_id={} -> cleanup until {}",
                        arguments.mode,
                        row.id,
                        cleanup_after.to_rfc3339()
                    );
                    if arguments.mode == Mode::Apply
                        && move_pending_to_cleanup(database, row.id, now, cleanup_after).await?
                    {
                        stats.moved_to_cleanup += 1;
                    }
                }
                DrainPlan::DeleteCleanup => {
                    println!("{:?} cleanup file_id={} -> delete", arguments.mode, row.id);
                    if arguments.mode == Mode::Apply
                        && delete_cleanup_reservation(database, storage, row.id, now).await?
                    {
                        stats.deleted_cleanup += 1;
                    }
                }
                DrainPlan::WaitUntil(until) => {
                    stats.waiting += 1;
                    println!(
                        "{:?} waiting file_id={} until={}",
                        arguments.mode,
                        row.id,
                        until.to_rfc3339()
                    );
                }
            }
        }
    }

    let remaining = sys_file::Entity::find()
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .count(database)
        .await?;
    println!(
        "reservation drain summary: scanned={} normalized_ready={} moved_to_cleanup={} deleted_cleanup={} waiting={} remaining={}",
        stats.scanned,
        stats.normalized_ready,
        stats.moved_to_cleanup,
        stats.deleted_cleanup,
        stats.waiting,
        remaining
    );
    if arguments.mode == Mode::Apply && remaining != 0 {
        return Err(
            format!("仍有 {remaining} 条旧上传预留；等待清理宽限期结束后重新执行同一命令").into(),
        );
    }
    Ok(())
}

fn plan_legacy_reservation(
    row: &sys_file::Model,
    now: DateTime<Utc>,
) -> Result<DrainPlan, DynError> {
    match row.upload_status.as_str() {
        sys_file::Model::UPLOAD_STATUS_READY => {
            validate_sha256(&row.file_sha256)
                .map_err(|error| format!("ready 文件 {} 的 SHA-256 无效: {error}", row.id))?;
            Ok(DrainPlan::NormalizeReady)
        }
        sys_file::Model::UPLOAD_STATUS_PENDING => match row.reservation_expires_at {
            Some(expires_at) if expires_at <= now => Ok(DrainPlan::MovePendingToCleanup),
            Some(expires_at) => Ok(DrainPlan::WaitUntil(expires_at)),
            None => Err(format!("pending 文件 {} 缺少预留到期时间", row.id).into()),
        },
        sys_file::Model::UPLOAD_STATUS_CLEANUP => match row.reservation_expires_at {
            Some(expires_at) if expires_at <= now => Ok(DrainPlan::DeleteCleanup),
            Some(expires_at) => Ok(DrainPlan::WaitUntil(expires_at)),
            None => Err(format!("cleanup 文件 {} 缺少清理到期时间", row.id).into()),
        },
        status => Err(format!("文件 {} 使用未知上传状态: {status}", row.id).into()),
    }
}

async fn normalize_ready_reservation(
    database: &DatabaseConnection,
    row: &sys_file::Model,
) -> Result<bool, DynError> {
    let result = sys_file::Entity::update_many()
        .col_expr(
            sys_file::Column::DelFlag,
            Expr::value(sys_file::Model::DEL_FLAG_NORMAL),
        )
        .col_expr(
            sys_file::Column::ReservationToken,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            sys_file::Column::ReservationExpiresAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .col_expr(sys_file::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(sys_file::Column::Id.eq(row.id))
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
        .filter(sys_file::Column::FileSha256.eq(&row.file_sha256))
        .exec(database)
        .await?;
    if result.rows_affected == 1 {
        return Ok(true);
    }
    let current = sys_file::Entity::find_by_id(row.id).one(database).await?;
    if current.as_ref().is_some_and(|file| {
        file.del_flag == sys_file::Model::DEL_FLAG_NORMAL
            && file.upload_status == sys_file::Model::UPLOAD_STATUS_READY
            && file.file_sha256 == row.file_sha256
    }) {
        return Ok(false);
    }
    Err(format!("ready 文件 {} 的状态 CAS 失败", row.id).into())
}

async fn move_pending_to_cleanup(
    database: &DatabaseConnection,
    id: i64,
    now: DateTime<Utc>,
    cleanup_after: DateTime<Utc>,
) -> Result<bool, DynError> {
    let result = sys_file::Entity::update_many()
        .col_expr(
            sys_file::Column::UploadStatus,
            Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
        )
        .col_expr(
            sys_file::Column::ReservationToken,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            sys_file::Column::ReservationExpiresAt,
            Expr::value(cleanup_after),
        )
        .col_expr(sys_file::Column::UpdatedAt, Expr::value(now))
        .filter(sys_file::Column::Id.eq(id))
        .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
        .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
        .filter(sys_file::Column::ReservationExpiresAt.lte(now))
        .exec(database)
        .await?;
    if result.rows_affected == 1 {
        return Ok(true);
    }
    let current = sys_file::Entity::find_by_id(id).one(database).await?;
    if current.as_ref().is_some_and(|file| {
        file.del_flag == LEGACY_RESERVED_FLAG
            && file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP
    }) {
        return Ok(false);
    }
    Err(format!("pending 文件 {id} 的状态 CAS 失败").into())
}

async fn delete_cleanup_reservation(
    database: &DatabaseConnection,
    storage: &dyn ObjectStorage,
    id: i64,
    now: DateTime<Utc>,
) -> Result<bool, DynError> {
    let transaction = database.begin().await?;
    let operation: Result<bool, DynError> = async {
        let current = sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .lock(LockType::Update)
            .one(&transaction)
            .await?;
        let Some(current) = current else {
            return Ok(false);
        };
        if current.upload_status != sys_file::Model::UPLOAD_STATUS_CLEANUP
            || !current
                .reservation_expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(format!("cleanup 文件 {id} 在锁定后已改变状态").into());
        }

        storage
            .delete(&current.bucket, &current.storage_path)
            .await
            .map_err(|error| format!("删除文件 {id} 对象失败: {error}"))?;
        let result = sys_file::Entity::delete_many()
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::DelFlag.eq(LEGACY_RESERVED_FLAG))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .exec(&transaction)
            .await?;
        if result.rows_affected != 1 {
            return Err(format!("cleanup 文件 {id} 的删除 CAS 失败").into());
        }
        Ok(true)
    }
    .await;

    match operation {
        Ok(deleted) => {
            transaction.commit().await?;
            Ok(deleted)
        }
        Err(error) => {
            if let Err(rollback_error) = transaction.rollback().await {
                return Err(
                    format!("{error}；同时回滚 cleanup 文件 {id} 失败: {rollback_error}").into(),
                );
            }
            Err(error)
        }
    }
}

async fn database_utc_now(database: &DatabaseConnection) -> Result<DateTime<Utc>, DynError> {
    let row = database
        .query_one_raw(Statement::from_string(
            database.get_database_backend(),
            "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
        ))
        .await?
        .ok_or("数据库时钟查询没有返回结果")?;
    let now: chrono::NaiveDateTime = row.try_get("", "db_now")?;
    Ok(DateTime::from_naive_utc_and_offset(now, Utc))
}

fn cleanup_grace(storage: &dyn ObjectStorage) -> chrono::Duration {
    let late_completion_seconds =
        i64::try_from(storage.late_put_completion_bound().as_secs()).unwrap_or(i64::MAX / 2);
    chrono::Duration::seconds(
        MIN_CLEANUP_GRACE_SECONDS.max(late_completion_seconds.saturating_mul(2)),
    )
}

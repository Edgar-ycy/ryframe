impl UserImportService {
    async fn load_department_directory(&self, tenant_id: &str) -> AppResult<DepartmentDirectory> {
        let departments = DeptRepository
            .find_filtered(self.db.write(), tenant_id, None, None)
            .await?;
        Ok(DepartmentDirectory::from_departments(departments))
    }

    async fn prepare_batch(
        &self,
        actor: &ActorContext,
        directory: &DepartmentDirectory,
        rows: &[ExcelImportRow<UserImportData>],
        tenant_authorization_epoch: i32,
        requester_authorization_version: i32,
    ) -> AppResult<PreparedBatch> {
        let mut issues = Vec::new();
        let mut candidates = Vec::new();
        let mut batch_usernames = HashSet::new();

        for row in rows {
            let row_number = i32::try_from(row.row_number)
                .map_err(|_| AppError::Validation("Excel 行号超出支持范围".into()))?;
            let mut data = match &row.value {
                Ok(data) => data.clone(),
                Err(error) => {
                    issues.push(RowIssue::failed(row_number, "", "invalid_row", error));
                    continue;
                }
            };
            normalize_import_data(&mut data);
            if let Err(error) = data.validate() {
                issues.push(RowIssue::failed(
                    row_number,
                    &data.username,
                    "validation_failed",
                    &error.to_string(),
                ));
                continue;
            }
            let department = match directory.resolve(data.department_path.as_deref(), actor) {
                Ok(department) => department,
                Err(issue) => {
                    issues.push(RowIssue::failed(
                        row_number,
                        &data.username,
                        issue.code,
                        &issue.message,
                    ));
                    continue;
                }
            };
            if !batch_usernames.insert(data.username.clone()) {
                issues.push(RowIssue::skipped(
                    row_number,
                    &data.username,
                    "duplicate_in_file",
                    "同一批次中已出现相同用户名",
                ));
                continue;
            }
            candidates.push(ImportCandidate {
                row_number,
                data,
                department_id: department.id,
            });
        }

        let prepared = try_join_all(candidates.into_iter().map(|candidate| {
            let permits = self.hash_permits.clone();
            async move {
                let permit = permits
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::Internal("用户导入密码哈希并发控制器已关闭".into()))?;
                let activation_secret = format!("pending:{}", Uuid::new_v4());
                let password_hash = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    password::hash(&activation_secret)
                })
                .await
                .map_err(|error| AppError::Internal(format!("密码哈希任务异常结束: {error}")))??;
                Ok::<_, AppError>(PreparedUser {
                    candidate,
                    password_hash,
                })
            }
        }))
        .await?;

        Ok(PreparedBatch {
            users: prepared,
            issues,
            tenant_authorization_epoch,
            requester_authorization_version,
        })
    }

    async fn commit_batch(
        &self,
        import_id: i64,
        expected_offset: usize,
        next_offset: usize,
        mut prepared: PreparedBatch,
    ) -> AppResult<CommitBatchOutcome> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let import_snapshot = user_import_job::Entity::find_by_id(import_id)
            .one(self.db.write())
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, &import_snapshot.tenant_id, None)
            .await?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.cancel_requested {
            let now = UserImportRepository.database_utc_now(&transaction).await?;
            import.status = user_import_job::Model::STATUS_CANCELLED.to_owned();
            import.completed_at = Some(now);
            import.updated_at = now;
            UserImportRepository
                .save_in_txn(&transaction, import)
                .await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(CommitBatchOutcome::Committed);
        }
        if usize::try_from(import.processed_rows).ok() != Some(expected_offset) {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(CommitBatchOutcome::Committed);
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        let usernames = prepared
            .users
            .iter()
            .map(|item| item.candidate.data.username.clone())
            .collect::<Vec<_>>();
        let tenant = TenantRepository
            .lock_tenant_in_txn(&transaction, &import.tenant_id)
            .await?;
        let requester = UserRepository
            .find_by_id_for_update(&transaction, &import.tenant_id, import.requester_user_id)
            .await?;
        let authorization_changed = tenant.authorization_epoch
            != prepared.tenant_authorization_epoch
            || !tenant.is_available(now)
            || requester.as_ref().is_none_or(|user| {
                !user.is_enabled()
                    || user.authorization_version != prepared.requester_authorization_version
            });
        if authorization_changed {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(CommitBatchOutcome::AuthorizationChanged);
        }
        let existing = UserRepository
            .find_existing_usernames_in_txn(&transaction, &import.tenant_id, &usernames)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut new_users = Vec::new();
        for prepared_user in prepared.users {
            if existing.contains(&prepared_user.candidate.data.username) {
                prepared.issues.push(RowIssue::skipped(
                    prepared_user.candidate.row_number,
                    &prepared_user.candidate.data.username,
                    "username_exists",
                    "用户名已存在，未覆盖现有用户",
                ));
            } else {
                new_users.push(prepared_user);
            }
        }

        if !new_users.is_empty()
            && let Err(error) = TenantRepository
                .ensure_user_quota_for_batch_in_txn(
                    &transaction,
                    &import.tenant_id,
                    new_users.len(),
                )
                .await
        {
            if !matches!(error, AppError::Validation(_)) {
                return Err(error);
            }
            for user in new_users.drain(..) {
                prepared.issues.push(RowIssue::failed(
                    user.candidate.row_number,
                    &user.candidate.data.username,
                    "tenant_quota_exceeded",
                    "当前批次将超过租户用户配额",
                ));
            }
        }

        let user_models = new_users
            .into_iter()
            .map(|prepared| build_user_model(&import.tenant_id, prepared, now))
            .collect::<AppResult<Vec<_>>>()?;
        let success_count = i32::try_from(user_models.len())
            .map_err(|_| AppError::Internal("用户导入成功计数溢出".into()))?;
        let skipped_count = i32::try_from(
            prepared
                .issues
                .iter()
                .filter(|issue| issue.outcome == user_import_row_result::Model::OUTCOME_SKIPPED)
                .count(),
        )
        .map_err(|_| AppError::Internal("用户导入跳过计数溢出".into()))?;
        let failure_count = i32::try_from(
            prepared
                .issues
                .iter()
                .filter(|issue| issue.outcome == user_import_row_result::Model::OUTCOME_FAILED)
                .count(),
        )
        .map_err(|_| AppError::Internal("用户导入失败计数溢出".into()))?;

        UserRepository
            .insert_many_in_txn(&transaction, &import.tenant_id, user_models)
            .await?;
        let row_models = prepared
            .issues
            .into_iter()
            .map(|issue| issue.into_model(&import.tenant_id, import_id, now))
            .collect::<AppResult<Vec<_>>>()?;
        UserImportRepository
            .insert_row_results_in_txn(&transaction, row_models)
            .await?;
        import.processed_rows = i32::try_from(next_offset)
            .map_err(|_| AppError::Internal("用户导入进度计数溢出".into()))?;
        import.success_count = import.success_count.saturating_add(success_count);
        import.skipped_count = import.skipped_count.saturating_add(skipped_count);
        import.failure_count = import.failure_count.saturating_add(failure_count);
        import.updated_at = now;
        import.last_error = None;
        UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(CommitBatchOutcome::Committed)
    }
}

struct PreparedUser {
    candidate: ImportCandidate,
    password_hash: String,
}

struct PreparedBatch {
    users: Vec<PreparedUser>,
    issues: Vec<RowIssue>,
    tenant_authorization_epoch: i32,
    requester_authorization_version: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitBatchOutcome {
    Committed,
    AuthorizationChanged,
}

struct RowIssue {
    row_number: i32,
    username: String,
    outcome: &'static str,
    code: String,
    message: String,
}

impl RowIssue {
    fn failed(row_number: i32, username: &str, code: &str, message: &str) -> Self {
        Self::new(
            row_number,
            username,
            user_import_row_result::Model::OUTCOME_FAILED,
            code,
            message,
        )
    }

    fn skipped(row_number: i32, username: &str, code: &str, message: &str) -> Self {
        Self::new(
            row_number,
            username,
            user_import_row_result::Model::OUTCOME_SKIPPED,
            code,
            message,
        )
    }

    fn new(
        row_number: i32,
        username: &str,
        outcome: &'static str,
        code: &str,
        message: &str,
    ) -> Self {
        Self {
            row_number,
            username: truncate_utf8(username, 64),
            outcome,
            code: truncate_utf8(code, 64),
            message: truncate_utf8(message, 500),
        }
    }

    fn into_model(
        self,
        tenant_id: &str,
        import_job_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<user_import_row_result::Model> {
        Ok(user_import_row_result::Model {
            id: next_id()?,
            tenant_id: tenant_id.to_owned(),
            import_job_id,
            row_number: self.row_number,
            username_snapshot: self.username,
            outcome: self.outcome.to_owned(),
            code: self.code,
            message: self.message,
            created_at: now,
        })
    }
}

#[derive(Serialize)]
struct UserImportReportRow {
    row_number: i32,
    username: String,
    outcome: String,
    code: String,
    message: String,
}

impl UserImportReportRow {
    const fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("row_number", "行号"),
            ("username", "用户名"),
            ("outcome", "结果"),
            ("code", "代码"),
            ("message", "说明"),
        ]
    }
}

fn build_user_model(
    tenant_id: &str,
    prepared: PreparedUser,
    now: DateTime<Utc>,
) -> AppResult<user::Model> {
    Ok(user::Model {
        id: next_id()?,
        tenant_id: tenant_id.to_owned(),
        username: prepared.candidate.data.username,
        password_hash: prepared.password_hash,
        nickname: prepared.candidate.data.nickname,
        email: prepared.candidate.data.email,
        phone: prepared.candidate.data.phone.unwrap_or_default(),
        avatar: None,
        avatar_file_id: None,
        preferred_locale: None,
        status: user::Model::STATUS_PENDING_ACTIVATION.to_owned(),
        authorization_version: 1,
        dept_id: Some(prepared.candidate.department_id),
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
        created_at: now,
        updated_at: now,
    })
}

fn normalize_import_data(data: &mut UserImportData) {
    data.username = data.username.trim().to_owned();
    data.nickname = data.nickname.trim().to_owned();
    data.email = data.email.trim().to_owned();
    data.phone = data
        .phone
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    data.department_path = data
        .department_path
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
}

fn department_is_visible(actor: &ActorContext, dept_id: i64) -> bool {
    if actor.is_super_admin || actor.data_scope == DataScope::All {
        return true;
    }
    match actor.data_scope {
        DataScope::All => true,
        DataScope::SelfOnly => false,
        DataScope::Dept => actor.dept_id == Some(dept_id),
        DataScope::DeptAndChildren | DataScope::Custom => actor.custom_dept_ids.contains(&dept_id),
    }
}

fn normalize_status(value: Option<&str>) -> AppResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !matches!(
        value,
        user_import_job::Model::STATUS_PENDING
            | user_import_job::Model::STATUS_RUNNING
            | user_import_job::Model::STATUS_SUCCEEDED
            | user_import_job::Model::STATUS_PARTIAL
            | user_import_job::Model::STATUS_FAILED
            | user_import_job::Model::STATUS_CANCELLED
    ) {
        return Err(AppError::Validation("用户导入状态筛选无效".into()));
    }
    Ok(Some(value.to_owned()))
}

fn validate_sha256(name: &str, value: &str) -> AppResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Validation(format!("{name}摘要格式无效")));
    }
    Ok(())
}

fn is_terminal_authorization_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_) | AppError::Authorization(_) | AppError::NotFound(_)
    )
}

fn is_terminal_import_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_)
            | AppError::Authentication(_)
            | AppError::Authorization(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::PayloadTooLarge(_)
    )
}

fn truncate_error(value: &str) -> String {
    truncate_utf8(value, 4_000)
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut end = maximum_bytes.saturating_sub('…'.len_utf8());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", &value[..end])
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

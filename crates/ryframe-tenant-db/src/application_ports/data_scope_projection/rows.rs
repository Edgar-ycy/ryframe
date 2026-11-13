use super::*;

pub(super) async fn scan_projection(
    source: &DatabaseTransaction,
    tenant_id: &str,
    source_token: Option<&str>,
    copy_to: Option<(&DatabaseTransaction, &str)>,
) -> AppResult<(i64, i64, String)> {
    let mut hasher = Sha256::new();
    let owner_count = scan_owners(source, tenant_id, source_token, copy_to, &mut hasher).await?;
    let dept_count =
        scan_departments(source, tenant_id, source_token, copy_to, &mut hasher).await?;
    Ok((owner_count, dept_count, hex::encode(hasher.finalize())))
}

async fn scan_owners(
    source: &DatabaseTransaction,
    tenant_id: &str,
    source_token: Option<&str>,
    copy_to: Option<(&DatabaseTransaction, &str)>,
    hasher: &mut Sha256,
) -> AppResult<i64> {
    let mut cursor = 0_i64;
    let mut count = 0_i64;
    loop {
        let rows = OwnerRow::find_by_statement(owner_query(tenant_id, source_token, cursor))
            .all(source)
            .await
            .db()?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            hasher.update(format!("u:{}:{:?}\n", row.id, row.dept_id));
        }
        if let Some((target, token)) = copy_to {
            insert_owner_batch(target, tenant_id, token, &rows).await?;
        }
        count += i64::try_from(rows.len()).unwrap_or(i64::MAX);
        cursor = rows.last().map(|row| row.id).unwrap_or(cursor);
        if rows.len() < usize::try_from(PAGE_SIZE).unwrap_or(usize::MAX) {
            break;
        }
    }
    Ok(count)
}

async fn scan_departments(
    source: &DatabaseTransaction,
    tenant_id: &str,
    source_token: Option<&str>,
    copy_to: Option<(&DatabaseTransaction, &str)>,
    hasher: &mut Sha256,
) -> AppResult<i64> {
    let mut cursor = 0_i64;
    let mut count = 0_i64;
    loop {
        let rows = DeptRow::find_by_statement(dept_query(tenant_id, source_token, cursor))
            .all(source)
            .await
            .db()?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            hasher.update(format!("d:{}:{}\n", row.id, row.ancestors));
        }
        if let Some((target, token)) = copy_to {
            insert_dept_batch(target, tenant_id, token, &rows).await?;
        }
        count += i64::try_from(rows.len()).unwrap_or(i64::MAX);
        cursor = rows.last().map(|row| row.id).unwrap_or(cursor);
        if rows.len() < usize::try_from(PAGE_SIZE).unwrap_or(usize::MAX) {
            break;
        }
    }
    Ok(count)
}

fn owner_query(tenant_id: &str, token: Option<&str>, cursor: i64) -> Statement {
    projection_query(
        tenant_id,
        token,
        cursor,
        "SELECT id, dept_id FROM sys_user \
         WHERE tenant_id = ? AND del_flag = '0' AND id > ? ORDER BY id LIMIT ?",
        "SELECT user_id AS id, dept_id FROM biz_data_scope_owner \
         WHERE tenant_id = ? AND build_token = ? AND user_id > ? ORDER BY user_id LIMIT ?",
    )
}

fn dept_query(tenant_id: &str, token: Option<&str>, cursor: i64) -> Statement {
    projection_query(
        tenant_id,
        token,
        cursor,
        "SELECT id, ancestors FROM sys_dept \
         WHERE tenant_id = ? AND del_flag = '0' AND id > ? ORDER BY id LIMIT ?",
        "SELECT dept_id AS id, ancestors FROM biz_data_scope_dept \
         WHERE tenant_id = ? AND build_token = ? AND dept_id > ? ORDER BY dept_id LIMIT ?",
    )
}

fn projection_query(
    tenant_id: &str,
    token: Option<&str>,
    cursor: i64,
    control_sql: &str,
    target_sql: &str,
) -> Statement {
    let mut values = vec![tenant_id.to_string().into()];
    let sql = if let Some(token) = token {
        values.push(token.to_string().into());
        target_sql
    } else {
        control_sql
    };
    values.extend([cursor.into(), PAGE_SIZE.into()]);
    Statement::from_sql_and_values(DbBackend::MySql, sql, values)
}

async fn insert_owner_batch(
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    rows: &[OwnerRow],
) -> AppResult<()> {
    let placeholders = std::iter::repeat_n("(?, ?, ?, ?)", rows.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO `biz_data_scope_owner` \
         (`tenant_id`, `build_token`, `user_id`, `dept_id`) VALUES {placeholders}"
    );
    let mut values = Vec::with_capacity(rows.len().saturating_mul(4));
    for row in rows {
        values.extend([
            tenant_id.to_string().into(),
            token.to_string().into(),
            row.id.into(),
            row.dept_id
                .map(Value::from)
                .unwrap_or_else(|| Value::from(None::<i64>)),
        ]);
    }
    target
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .db()?;
    Ok(())
}

async fn insert_dept_batch(
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    rows: &[DeptRow],
) -> AppResult<()> {
    let placeholders = std::iter::repeat_n("(?, ?, ?, ?)", rows.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO `biz_data_scope_dept` \
         (`tenant_id`, `build_token`, `dept_id`, `ancestors`) VALUES {placeholders}"
    );
    let mut values = Vec::with_capacity(rows.len().saturating_mul(4));
    for row in rows {
        values.extend([
            tenant_id.to_string().into(),
            token.to_string().into(),
            row.id.into(),
            row.ancestors.clone().into(),
        ]);
    }
    target
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .db()?;
    Ok(())
}

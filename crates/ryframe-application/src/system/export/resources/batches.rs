use super::*;

impl ExportService {
    pub(super) async fn append_export_batch(
        &self,
        writer: &mut dyn SpreadsheetWriter,
        actor: &ActorContext,
        selection: &ExportSelection,
        window: ExportCursorWindow,
    ) -> AppResult<Option<AppendedBatch>> {
        match selection {
            ExportSelection::Users(filters) => {
                let batch = self
                    .users
                    .find_export_batch(
                        actor,
                        UserQueryFilter {
                            username: filters.username(),
                            phone: filters.phone(),
                            status: filters.status(),
                            dept_id: filters.dept_id(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|user| {
                        serde_json::json!({
                            "user_id": user.id,
                            "username": user.username,
                            "nickname": user.nickname,
                            "email": user.email,
                            "phone": user.phone,
                            "dept_name": user.dept_name,
                            "status": user.status,
                            "remark": user.remark,
                            "created_at": user.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Roles(filters) => {
                let batch = self
                    .roles
                    .find_export_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "role_id": item.id, "role_name": item.name, "role_code": item.code,
                            "data_scope": item.data_scope, "status": item.status, "sort": item.sort,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Posts(filters) => {
                let batch = self
                    .posts
                    .find_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "post_id": item.id, "name": item.name, "code": item.code,
                            "sort": item.sort, "status": item.status, "remark": item.remark,
                            "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::Configs(filters) => {
                let batch = self
                    .configs
                    .find_export_batch(actor, filters.name(), filters.key(), window)
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "name": item.name, "key": item.key, "value": item.value,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::DictTypes(filters) => {
                let batch = self
                    .dicts
                    .find_type_export_batch(
                        actor,
                        filters.name(),
                        filters.code(),
                        filters.status(),
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "name": item.name, "code": item.code, "status": item.status,
                            "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::OperLogs(filters) => {
                let batch = self
                    .oper_logs
                    .find_export_batch(
                        actor,
                        OperLogFilter {
                            oper_name: filters.oper_name(),
                            status: filters.status(),
                            begin_time: filters.begin_time(),
                            end_time: filters.end_time(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "title": item.title, "business_type": item.business_type,
                            "oper_name": item.oper_name, "oper_url": item.oper_url,
                            "oper_ip": item.oper_ip, "status": item.status,
                            "cost_time": item.cost_time, "oper_time": item.oper_time,
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
            ExportSelection::LoginLogs(filters) => {
                let batch = self
                    .login_infos
                    .find_export_batch(
                        actor,
                        LoginInfoFilter {
                            user_name: filters.user_name(),
                            status: filters.status(),
                            begin_time: filters.begin_time(),
                            end_time: filters.end_time(),
                        },
                        window,
                    )
                    .await?;
                let Some(last_id) = last_batch_id(&batch, window, |item| &item.id)? else {
                    return Ok(None);
                };
                let progress = append_rows(
                    writer,
                    batch.into_iter().map(|item| {
                        serde_json::json!({
                            "user_name": item.user_name, "ipaddr": item.ipaddr,
                            "login_location": item.login_location, "browser": item.browser,
                            "os": item.os, "status": item.status, "msg": item.msg,
                            "login_time": item.login_time,
                        })
                    }),
                )?;
                Ok(Some(AppendedBatch { last_id, progress }))
            }
        }
    }
}

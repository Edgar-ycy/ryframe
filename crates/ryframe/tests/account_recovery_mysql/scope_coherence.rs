use ryframe_application::system::identity::{CreateUserParams, UpdateUserParams};
use ryframe_application::{AuthService, AuthorizationCache, PrincipalResolver};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::{administrator, imported_accounts, message, role_assignment, services_with_cache};

pub(super) async fn verify(
    database: &DatabaseConnection,
    cache: &AuthorizationCache,
    auth: &AuthService,
    claims: &Claims,
) -> Result<(), String> {
    let (users, _) = services_with_cache(database.clone(), cache.clone())?;
    let actor = administrator();
    let mut principal = auth.resolve_principal(claims).await.map_err(message)?;
    assert!(
        cache
            .lookup_snapshot("system", 2)
            .await
            .map_err(message)?
            .snapshot
            .is_some()
    );
    let created = users
        .create(
            &actor,
            CreateUserParams {
                username: "scope-cache-subject",
                nickname: "授权缓存回归",
                email: "scope@example.com",
                phone: "",
                dept_id: None,
                role_ids: vec![],
            },
        )
        .await
        .map_err(message)?;
    principal = advanced(database, cache, auth, claims, &principal, 1).await?;
    let id = created.id.parse::<i64>().map_err(message)?;
    users
        .update(
            &actor,
            UpdateUserParams {
                id,
                nickname: "授权缓存更新",
                email: "scope@example.com",
                phone: "",
                dept_id: None,
            },
        )
        .await
        .map_err(message)?;
    principal = advanced(database, cache, auth, claims, &principal, 1).await?;
    users.delete(&actor, id).await.map_err(message)?;
    principal = advanced(database, cache, auth, claims, &principal, 1).await?;
    rollback(database, cache, &principal).await?;
    imports(database, cache, auth, claims, users, principal).await
}

async fn advanced(
    database: &DatabaseConnection,
    cache: &AuthorizationCache,
    auth: &AuthService,
    claims: &Claims,
    before: &RequestPrincipal,
    increments: i32,
) -> Result<RequestPrincipal, String> {
    let lookup = cache.lookup_snapshot("system", 2).await.map_err(message)?;
    assert_eq!(
        lookup.tenant_authorization_epoch,
        Some(before.tenant_authorization_epoch + increments)
    );
    assert!(lookup.snapshot.is_none(), "其他主体的旧热点快照必须失效");
    let after = auth.resolve_principal(claims).await.map_err(message)?;
    assert_eq!(
        after.tenant_authorization_epoch,
        before.tenant_authorization_epoch + increments
    );
    assert_eq!(
        after.data_scope_version,
        before.data_scope_version + i64::from(increments)
    );
    assert_eq!(after.actor.data_scope_version, after.data_scope_version);
    assert_eq!(
        after.user_authorization_version,
        before.user_authorization_version
    );
    assert_eq!(after.actor.user_id, before.actor.user_id);
    assert_eq!(after.actor.tenant_id, before.actor.tenant_id);
    assert_eq!(
        mirror(database, after.tenant_authorization_epoch).await?,
        Some(ryframe_application::AuthorizationMirrorUpdate::Tenant {
            tenant_id: "system".into(),
            authorization_epoch: after.tenant_authorization_epoch,
        })
    );
    assert!(
        cache
            .lookup_snapshot("system", 2)
            .await
            .map_err(message)?
            .snapshot
            .is_some()
    );
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT authorization_epoch,data_scope_version FROM sys_tenant WHERE tenant_id='system'"))
        .await.map_err(message)?.ok_or("缺少本次租户")?;
    assert_eq!(
        row.try_get::<i32>("", "authorization_epoch")
            .map_err(message)?,
        after.tenant_authorization_epoch
    );
    assert_eq!(
        row.try_get::<i64>("", "data_scope_version")
            .map_err(message)?,
        after.data_scope_version
    );
    Ok(after)
}

async fn rollback(
    database: &DatabaseConnection,
    cache: &AuthorizationCache,
    principal: &RequestPrincipal,
) -> Result<(), String> {
    let port = ryframe_db::application_ports::users::write(
        ryframe_db::ControlDatabaseCluster::single(database.clone()),
        cache.clone(),
    );
    let transaction = port.begin().await.map_err(message)?;
    transaction
        .record_data_scope_change("system")
        .await
        .map_err(message)?;
    transaction
        .increment_authorization_epoch("system")
        .await
        .map_err(message)?;
    transaction.rollback().await.map_err(message)?;
    assert!(
        mirror(database, principal.tenant_authorization_epoch + 1)
            .await?
            .is_none()
    );
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT authorization_epoch,data_scope_version FROM sys_tenant WHERE tenant_id='system'"))
        .await.map_err(message)?.ok_or("缺少本次租户")?;
    assert_eq!(
        row.try_get::<i32>("", "authorization_epoch")
            .map_err(message)?,
        principal.tenant_authorization_epoch
    );
    assert_eq!(
        row.try_get::<i64>("", "data_scope_version")
            .map_err(message)?,
        principal.data_scope_version
    );
    assert!(
        cache
            .lookup_snapshot("system", 2)
            .await
            .map_err(message)?
            .snapshot
            .is_some()
    );
    Ok(())
}

async fn imports(
    database: &DatabaseConnection,
    cache: &AuthorizationCache,
    auth: &AuthService,
    claims: &Claims,
    users: ryframe_application::system::identity::UserService,
    mut principal: RequestPrincipal,
) -> Result<(), String> {
    use futures_util::FutureExt;
    use ryframe_application::{UserImportPolicy, system::identity::UserImportService};
    use std::sync::Arc;
    let (mut state, lifetime) = role_assignment::state(database).await?;
    let service = Arc::new(UserImportService::new(
        state.services.operations.job_queue.clone(),
        Arc::new(users),
        state.services.content.file.clone(),
        ryframe::boot::spreadsheet::document_processor(),
        ryframe_db::application_ports::users::import(
            ryframe_db::ControlDatabaseCluster::single(database.clone()),
            cache.clone(),
        ),
        cache.clone(),
        UserImportPolicy::new(10 * 1024 * 1024, 100, 1, 5, 1).map_err(message)?,
    ));
    Arc::make_mut(&mut state.services).identity.user_import = service.clone();
    let result =
        std::panic::AssertUnwindSafe(async {
            for (key, inserted) in [('a', true), ('b', false)] {
                let job = submit_import(database, &service, key).await?;
                let id = job.parse::<i64>().map_err(message)?;
                imported_accounts::execute_claimed_import(database, &state, id).await?;
                let job = service.get(&administrator(), id).await.map_err(message)?;
                if inserted {
                    assert_eq!(job.success_count, 2);
                    assert_eq!(job.skipped_count, 0);
                    principal = advanced(database, cache, auth, claims, &principal, 2).await?;
                } else {
                    assert_eq!(job.success_count, 0);
                    assert_eq!(job.skipped_count, 2);
                    assert_eq!(job.failure_count, 0);
                    assert!(
                        mirror(database, principal.tenant_authorization_epoch + 1)
                            .await?
                            .is_none()
                    );
                    let current = auth.resolve_principal(claims).await.map_err(message)?;
                    assert_eq!(current.data_scope_version, principal.data_scope_version);
                    assert_eq!(
                        current.tenant_authorization_epoch,
                        principal.tenant_authorization_epoch
                    );
                    super::mysql::require_count(database,
                    "SELECT data_scope_version AS value FROM sys_tenant WHERE tenant_id='system'",
                    principal.data_scope_version).await?;
                    super::mysql::require_count(database,
                    "SELECT authorization_epoch AS value FROM sys_tenant WHERE tenant_id='system'",
                    i64::from(principal.tenant_authorization_epoch)).await?;
                    assert!(
                        cache
                            .lookup_snapshot("system", 2)
                            .await
                            .map_err(message)?
                            .snapshot
                            .is_some()
                    );
                }
            }
            Ok(())
        })
        .catch_unwind()
        .await;
    drop(state);
    match result {
        Ok(Ok(())) => {
            drop(lifetime);
            Ok(())
        }
        Ok(Err(error)) => {
            lifetime.preserve();
            Err(error)
        }
        Err(payload) => {
            lifetime.preserve();
            std::panic::resume_unwind(payload)
        }
    }
}

async fn mirror(
    database: &DatabaseConnection,
    epoch: i32,
) -> Result<Option<ryframe_application::AuthorizationMirrorUpdate>, String> {
    let row = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT CAST(payload AS CHAR) AS payload FROM sys_outbox_event WHERE tenant_id='system' AND event_type=? AND dedupe_key=?",
        [ryframe_application::AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE.into(),
            format!("authorization-mirror:system:tenant:system:{epoch}").into()]))
        .await.map_err(message)?;
    row.map(|row| {
        let payload: String = row.try_get("", "payload").map_err(message)?;
        serde_json::from_str(&payload).map_err(message)
    })
    .transpose()
}

async fn submit_import(
    database: &DatabaseConnection,
    service: &ryframe_application::system::identity::UserImportService,
    key: char,
) -> Result<String, String> {
    use ryframe_application::system::identity::RequestUserImportCommand;
    use sha2::{Digest, Sha256};
    let bytes = imported_accounts::workbook(database).await?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let bytes = service.validate_source(bytes).await.map_err(message)?;
    let uploaded = service
        .upload_source(&administrator(), "accounts.xlsx".into(), bytes)
        .await
        .map_err(message)?;
    let outcome = service
        .request(
            &administrator(),
            RequestUserImportCommand {
                idempotency_key_hash: key.to_string().repeat(64),
                source_file_id: uploaded.file_id.parse::<i64>().map_err(message)?,
                source_name: "accounts.xlsx".into(),
                source_sha256: sha256,
            },
        )
        .await
        .map_err(message)?;
    assert!(outcome.inserted);
    Ok(outcome.job.id)
}

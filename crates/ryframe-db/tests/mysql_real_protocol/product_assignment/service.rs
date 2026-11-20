use std::sync::{Arc, Mutex};

use ryframe_application::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AUTHORIZATION_CHANGED_REDIS_CHANNEL,
    AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditRequestContext,
    AuthorizationCache, AuthorizationChangePublisher, AuthorizationChangedEvent,
    CacheAvailabilityPolicy, scope_audit_request,
    system::{
        operations::{OperLogStatus, RecordOperLogCommand},
        platform::{
            ApplyProductChangeCommand, CapabilityOverrideInput, ProductChangeTarget,
            ProductContextVo, ProductService,
        },
    },
};
use ryframe_db::{
    ControlDatabaseCluster,
    application_ports::product,
    entities::{
        outbox_event,
        tenant::{capability_override, operation_lease, product_plan},
    },
    repositories::{ProductRepository, TenantOperationLeaseRepository},
};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait};

use super::super::{
    bootstrap_hashes, db_error, execute, install_test_database_id_generator, run_mysql_test,
};

const TENANT: &str = "product-fixture";
const ORIGINAL_VERSION: i64 = 1;
const NEXT_VERSION: i64 = 72;
const FOREIGN_OWNER: &str = "product-test-foreign";
const NEW_REASON: &str = "新的分配理由";
const SAME_AUDIT: &str = "product-same";
const REASON_AUDIT: &str = "product-reason";
const SWITCH_AUDIT: &str = "product-switch";

#[derive(Debug, PartialEq)]
struct AssignmentImage {
    assignment: product_plan::Model,
    overrides: Vec<capability_override::Model>,
    runtime_epoch: i64,
    authorization_epoch: i32,
    context: serde_json::Value,
}

#[derive(Default)]
struct PublishedEvents(Mutex<Vec<(String, String, i32)>>);

#[async_trait::async_trait]
impl AuthorizationChangePublisher for PublishedEvents {
    async fn publish(&self, channel: &str, payload: &str) -> Result<(), String> {
        let event: AuthorizationChangedEvent =
            serde_json::from_str(payload).map_err(|error| error.to_string())?;
        self.0.lock().unwrap().push((
            channel.to_owned(),
            event.tenant_id,
            event.authorization_epoch,
        ));
        Ok(())
    }
}

struct Fixture {
    database: DatabaseConnection,
    service: ProductService,
    actor: ActorContext,
    events: Arc<PublishedEvents>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_product_assignment_is_audited_without_advancing_runtime() {
    run_mysql_test("product-assignment", |database| async move {
        let fixture = Fixture::prepare(database).await?;
        let original = fixture.image().await?;
        assert_eq!(original.overrides.len(), 2);
        assert_eq!(original.overrides[0].capability_code, "system.message");
        assert_eq!(original.assignment.plan_version_id, ORIGINAL_VERSION);
        let unchanged_command = fixture.preview(ORIGINAL_VERSION).await?;
        assert_eq!(
            unchanged_command.target.overrides[0].capability_code,
            "system.notice"
        );

        apply_same_configuration(&fixture, &unchanged_command, original).await?;
        reject_invalid_submissions(&fixture, &unchanged_command).await?;
        switch_version_and_reject_old_preview(&fixture).await
    })
    .await;
}

async fn apply_same_configuration(
    fixture: &Fixture,
    command: &ApplyProductChangeCommand,
    mut expected: AssignmentImage,
) -> Result<(), String> {
    let unchanged = fixture
        .submit(SAME_AUDIT, command.clone())
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(serde_json::to_value(unchanged).unwrap(), expected.context);
    assert_eq!(fixture.image().await?, expected);
    assert_eq!(fixture.lease().await?, None);
    fixture.check_audits(&[SAME_AUDIT]).await?;
    assert!(fixture.events.0.lock().unwrap().is_empty());

    let mut with_reason = command.clone();
    with_reason.reason = Some(NEW_REASON.into());
    let updated = fixture
        .submit(REASON_AUDIT, with_reason)
        .await
        .map_err(|error| error.to_string())?;
    expected.assignment.change_reason = Some(NEW_REASON.into());
    assert_eq!(serde_json::to_value(updated).unwrap(), expected.context);
    assert_eq!(fixture.image().await?, expected);
    assert_eq!(fixture.lease().await?, None);
    fixture.check_audits(&[SAME_AUDIT, REASON_AUDIT]).await?;
    assert!(fixture.events.0.lock().unwrap().is_empty());
    Ok(())
}

async fn reject_invalid_submissions(
    fixture: &Fixture,
    command: &ApplyProductChangeCommand,
) -> Result<(), String> {
    let expected = fixture.image().await?;
    let mut wrong_hash = command.clone();
    wrong_hash.plan_hash.insert(0, '0');
    assert!(matches!(
        fixture.submit("product-wrong-hash", wrong_hash).await,
        Err(AppError::Conflict(message)) if message.contains("计划哈希无效")
    ));
    assert_eq!(fixture.image().await?, expected);
    assert_eq!(fixture.lease().await?, None);
    fixture.check_audits(&[SAME_AUDIT, REASON_AUDIT]).await?;
    assert!(fixture.events.0.lock().unwrap().is_empty());

    execute(
        &fixture.database,
        "INSERT INTO sys_tenant_operation_lease \
         (tenant_id,owner_token,operation,resource_type,resource_id,expires_at,created_at,updated_at) \
         VALUES ('product-fixture','product-test-foreign','product.change','product_plan_version',\
         '72',UTC_TIMESTAMP(6)+INTERVAL 5 MINUTE,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))",
    )
    .await?;
    let lease = fixture.lease().await?.ok_or("缺少已登记的外部租约")?;
    assert_eq!(lease.owner_token, FOREIGN_OWNER);
    assert!(matches!(
        fixture
            .submit("product-foreign-lease", command.clone())
            .await,
        Err(AppError::TenantOperationConflict(_))
    ));
    assert_eq!(fixture.image().await?, expected);
    assert_eq!(fixture.lease().await?, Some(lease));
    fixture.check_audits(&[SAME_AUDIT, REASON_AUDIT]).await?;
    assert!(fixture.events.0.lock().unwrap().is_empty());

    let transaction = fixture.database.begin().await.map_err(db_error)?;
    assert!(
        TenantOperationLeaseRepository
            .release_in_txn(&transaction, TENANT, FOREIGN_OWNER)
            .await
            .map_err(|error| error.to_string())?
    );
    transaction.commit().await.map_err(db_error)?;
    assert_eq!(fixture.lease().await?, None);
    Ok(())
}

async fn switch_version_and_reject_old_preview(fixture: &Fixture) -> Result<(), String> {
    let before = fixture.image().await?;
    let mut command = fixture.preview(NEXT_VERSION).await?;
    command.reason = Some("切换已发布版本".into());
    let updated = fixture
        .submit(SWITCH_AUDIT, command.clone())
        .await
        .map_err(|error| error.to_string())?;
    let after = fixture.image().await?;
    assert_eq!(after.assignment.plan_version_id, NEXT_VERSION);
    assert_eq!(after.assignment.change_reason, command.reason);
    assert_eq!(after.assignment.changed_by, Some(fixture.actor.user_id));
    assert_eq!(after.assignment.created_at, before.assignment.created_at);
    assert_eq!(after.overrides.len(), before.overrides.len());
    assert_eq!(
        after.context["capabilities"],
        before.context["capabilities"]
    );
    assert_eq!(after.runtime_epoch, before.runtime_epoch + 1);
    assert_eq!(after.authorization_epoch, before.authorization_epoch);
    assert_eq!(updated.plan_version_id, NEXT_VERSION.to_string());
    assert_eq!(updated.runtime_epoch, after.runtime_epoch.to_string());
    assert_eq!(serde_json::to_value(updated).unwrap(), after.context);
    assert_eq!(fixture.lease().await?, None);
    fixture
        .check_audits(&[SAME_AUDIT, REASON_AUDIT, SWITCH_AUDIT])
        .await?;
    let expected_events = [(
        AUTHORIZATION_CHANGED_REDIS_CHANNEL.to_owned(),
        TENANT.to_owned(),
        before.authorization_epoch,
    )];
    assert_eq!(
        fixture.events.0.lock().unwrap().as_slice(),
        &expected_events
    );

    assert!(matches!(
        fixture.submit("product-stale-preview", command).await,
        Err(AppError::StaleRuntimeEpoch(_))
    ));
    assert_eq!(fixture.image().await?, after);
    assert_eq!(fixture.lease().await?, None);
    fixture
        .check_audits(&[SAME_AUDIT, REASON_AUDIT, SWITCH_AUDIT])
        .await?;
    assert_eq!(
        fixture.events.0.lock().unwrap().as_slice(),
        &expected_events
    );
    Ok(())
}

impl Fixture {
    async fn prepare(database: DatabaseConnection) -> Result<Self, String> {
        install_test_database_id_generator()?;
        ryframe_db::migration::up(&database, Some(&bootstrap_hashes()))
            .await
            .map_err(db_error)?;
        for statement in [
            "INSERT INTO sys_tenant(id,tenant_id,name,status) \
             VALUES (71,'product-fixture','套餐验收租户','enabled')",
            "INSERT INTO sys_product_plan_version \
             (id,plan_id,version,name,description,status,created_by,published_by,published_at,created_at,updated_at) \
             SELECT 72,plan_id,2,'标准版 v2',description,status,created_by,published_by,published_at,created_at,updated_at \
             FROM sys_product_plan_version WHERE id=1",
            "INSERT INTO sys_tenant_product_plan \
             (tenant_id,plan_version_id,changed_by,change_reason,created_at,updated_at) \
             VALUES ('product-fixture',1,29,'原分配理由','2026-01-01 01:02:03.123456','2026-01-01 02:03:04.123456')",
            "INSERT INTO sys_tenant_capability_override \
             (tenant_id,capability_code,enabled,variant_code,schema_version,config,reason,changed_by,created_at,updated_at) VALUES \
             ('product-fixture','system.notice',0,'standard',1,'{}','原公告覆盖理由',31,'2026-01-01 01:00:00.123456','2026-01-01 02:00:00.123456'), \
             ('product-fixture','system.message',0,'standard',1,'{}','原消息覆盖理由',32,'2026-01-01 01:00:01.123456','2026-01-01 02:00:01.123456')",
        ] {
            execute(&database, statement).await?;
        }
        let events = Arc::new(PublishedEvents::default());
        let cluster = ControlDatabaseCluster::single(database.clone());
        let service = ProductService::new(
            product::read(cluster.clone()),
            product::write(cluster),
            AuthorizationCache::new(
                None,
                Some(events.clone()),
                CacheAvailabilityPolicy::Optional,
            ),
        );
        let actor = ActorContext {
            user_id: 1,
            tenant_id: "system".into(),
            username: "admin".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::All,
            custom_dept_ids: Vec::new(),
            include_self: true,
            data_scope_version: 1,
            is_super_admin: true,
        };
        Ok(Self {
            database,
            service,
            actor,
            events,
        })
    }

    async fn image(&self) -> Result<AssignmentImage, String> {
        let bundle = ProductRepository
            .tenant_product(&self.database, TENANT)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("缺少套餐分配前后像")?;
        let context = self
            .service
            .effective_context(TENANT)
            .await
            .map_err(|error| error.to_string())?;
        Ok(AssignmentImage {
            assignment: bundle.assignment,
            overrides: bundle.overrides,
            runtime_epoch: bundle.tenant.runtime_epoch,
            authorization_epoch: bundle.tenant.authorization_epoch,
            context: serde_json::to_value(context).map_err(|error| error.to_string())?,
        })
    }

    async fn lease(&self) -> Result<Option<operation_lease::Model>, String> {
        operation_lease::Entity::find_by_id(TENANT.to_owned())
            .one(&self.database)
            .await
            .map_err(db_error)
    }

    async fn preview(&self, plan_version_id: i64) -> Result<ApplyProductChangeCommand, String> {
        let target = ProductChangeTarget {
            plan_version_id,
            overrides: ["system.notice", "system.message"]
                .into_iter()
                .map(|code| CapabilityOverrideInput {
                    capability_code: code.into(),
                    enabled: false,
                    variant_code: "standard".into(),
                    schema_version: 1,
                    config: serde_json::json!({}),
                })
                .collect(),
        };
        let preview = self
            .service
            .preview_change(&self.actor, TENANT, target.clone(), false)
            .await
            .map_err(|error| error.to_string())?;
        Ok(ApplyProductChangeCommand {
            target,
            preview_runtime_epoch: preview
                .runtime_epoch
                .parse::<i64>()
                .map_err(|e| e.to_string())?,
            plan_hash: preview.plan_hash,
            reason: None,
            capability_override_allowed: false,
        })
    }

    async fn submit(
        &self,
        event_id: &str,
        command: ApplyProductChangeCommand,
    ) -> AppResult<ProductContextVo> {
        let audit = AuditRequestContext::new(
            event_id.into(),
            format!("request-{event_id}"),
            self.actor.tenant_id.clone(),
            RecordOperLogCommand {
                title: "套餐分配".into(),
                business_type: "update".into(),
                method: "apply_tenant_change".into(),
                request_method: "POST".into(),
                oper_name: self.actor.username.clone(),
                oper_url: format!("/api/v1/platform/tenants/{TENANT}/product-changes"),
                oper_ip: "127.0.0.1".into(),
                oper_param: None,
                json_result: None,
                status: OperLogStatus::Success,
                error_msg: None,
                cost_time: 0,
            },
        )?;
        let result = scope_audit_request(
            audit.clone(),
            self.service.apply_change(&self.actor, TENANT, command),
        )
        .await;
        assert_eq!(audit.transaction_committed(), result.is_ok(), "{event_id}");
        result
    }

    async fn check_audits(&self, expected: &[&str]) -> Result<(), String> {
        let events = outbox_event::Entity::find()
            .filter(outbox_event::Column::EventType.is_in([
                AUDIT_OPERATION_OUTBOX_EVENT_TYPE,
                AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE,
            ]))
            .all(&self.database)
            .await
            .map_err(db_error)?;
        assert_eq!(events.len(), expected.len());
        for event_id in expected {
            let matching = events
                .iter()
                .filter(|row| {
                    row.event_type == AUDIT_OPERATION_OUTBOX_EVENT_TYPE
                        && row.dedupe_key.as_deref() == Some(*event_id)
                })
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1, "{event_id}");
            let row = matching[0];
            assert_eq!(row.aggregate_id, *event_id);
            assert_eq!(
                row.tenant_id.as_deref(),
                Some(self.actor.tenant_id.as_str())
            );
            let event: AuditOperationEvent =
                serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
            assert_eq!(event.event_id, *event_id);
            assert_eq!(event.request_id, format!("request-{event_id}"));
            assert_eq!(event.tenant_id, self.actor.tenant_id);
            assert!(matches!(event.command.status, OperLogStatus::Success));
            assert_eq!(event.command.error_msg, None);
        }
        Ok(())
    }
}

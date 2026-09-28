#[path = "../tenant_provisioning_support/fixtures.rs"]
mod fixtures;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope, ErrorCode};

use fixtures::policy_template;
use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::{
        authorization::AuthorizationMirrorTransaction,
        product::{
            ProductPlanRecord, ProductReadPort, ProductTransactionPort, ProductVersionSnapshot,
            ProvisioningCapabilityResources, TenantProductSnapshot,
        },
        tenants::{
            ProvisionTenantRecord, TENANT_STATUS_ENABLED, TENANT_STATUS_PROVISIONING,
            TenantAdminRecord, TenantAuthorizationTemplate, TenantBaseCatalogTemplate,
            TenantPersistencePort, TenantProductAssignmentRecord, TenantProvisionRequestRecord,
            TenantProvisioningIdentity, TenantProvisioningPlacement, TenantProvisioningPort,
            TenantProvisioningTemplate, TenantRecord, TenantTransaction,
        },
    },
    system::platform::{CreateTenantParams, ProductService, TenantService, TenantVo},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailAt {
    Load,
    Identity,
    Authorization,
    BaseCatalogs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Call {
    Begin,
    Lock,
    Product,
    Load,
    Identity,
    Authorization(TenantProvisioningIdentity, TenantAuthorizationTemplate),
    BaseCatalogs(TenantProvisioningIdentity, TenantBaseCatalogTemplate),
    AssignProduct,
    CreatePending,
    LockTenant,
    SyncResources,
    Activate,
    UpdateStatus,
    Commit(TransactionAuditMode),
    Rollback,
}

struct FakeState {
    calls: Mutex<Vec<Call>>,
    fail_at: Option<FailAt>,
}

impl FakeState {
    fn push(&self, call: Call) {
        self.calls.lock().expect("调用记录锁应可用").push(call);
    }

    fn fail(&self, stage: FailAt) -> AppResult<()> {
        if self.fail_at == Some(stage) {
            Err(AppError::Internal(format!("模拟 {stage:?} 失败")))
        } else {
            Ok(())
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("调用记录锁应可用").clone()
    }
}

struct FakePersistence {
    state: Arc<FakeState>,
    template: TenantProvisioningTemplate,
}

#[async_trait]
impl TenantPersistencePort for FakePersistence {
    async fn list(&self) -> AppResult<Vec<TenantRecord>> {
        unreachable!("本测试不读取租户列表")
    }

    async fn find<'a>(&'a self, tenant_id: &'a str) -> AppResult<Option<TenantRecord>> {
        Ok(Some(tenant(tenant_id, TENANT_STATUS_ENABLED)))
    }

    async fn begin(&self) -> AppResult<Box<dyn TenantTransaction>> {
        self.state.push(Call::Begin);
        Ok(Box::new(FakeTransaction {
            state: Arc::clone(&self.state),
            template: self.template.clone(),
        }))
    }
}

struct FakeTransaction {
    state: Arc<FakeState>,
    template: TenantProvisioningTemplate,
}

impl FakeTransaction {
    fn stage(&self, call: Call, stage: FailAt) -> AppResult<()> {
        self.state.push(call);
        self.state.fail(stage)
    }
}

#[async_trait]
impl PersistenceTransaction for FakeTransaction {
    async fn commit(self: Box<Self>, mode: TransactionAuditMode) -> AppResult<()> {
        self.state.push(Call::Commit(mode));
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.state.push(Call::Rollback);
        Ok(())
    }
}

#[async_trait]
impl ProductTransactionPort for FakeTransaction {
    async fn current_tenant_product<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> AppResult<TenantProductSnapshot> {
        unreachable!("本测试不读取现有租户产品")
    }

    async fn lock_assignable_version(&self, version_id: i64) -> AppResult<ProductVersionSnapshot> {
        self.state.push(Call::Product);
        Ok(ProductVersionSnapshot {
            plan_key: "default".into(),
            plan_name: "默认套餐".into(),
            plan_status: "1".into(),
            version_id,
            version: 1,
            version_status: "published".into(),
            capabilities: Vec::new(),
        })
    }

    async fn sync_capability_resources<'a>(
        &'a self,
        _tenant_id: &'a str,
        _resources: &'a ProvisioningCapabilityResources,
    ) -> AppResult<()> {
        self.state.push(Call::SyncResources);
        Ok(())
    }
}

#[async_trait]
impl TenantTransaction for FakeTransaction {
    fn product(&self) -> &dyn ProductTransactionPort {
        self
    }

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        unreachable!("本测试不访问授权镜像")
    }

    async fn lock_optional_tenant<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> AppResult<Option<TenantRecord>> {
        self.state.push(Call::Lock);
        Ok(None)
    }

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> AppResult<TenantRecord> {
        self.state.push(Call::LockTenant);
        Ok(tenant(tenant_id, TENANT_STATUS_PROVISIONING))
    }

    async fn lock_tenant_with_limits<'a>(
        &'a self,
        _tenant_id: &'a str,
        _max_users: i32,
        _max_roles: i32,
        _max_storage_mb: i64,
    ) -> AppResult<TenantRecord> {
        unreachable!("本测试不更新租户")
    }

    async fn lock_provision_request<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> AppResult<Option<TenantProvisionRequestRecord>> {
        unreachable!("本测试不恢复创建请求")
    }

    async fn load_provisioning_template(&self) -> AppResult<TenantProvisioningTemplate> {
        self.stage(Call::Load, FailAt::Load)?;
        Ok(self.template.clone())
    }

    async fn initialize_tenant_identity<'a>(
        &'a self,
        record: &'a ProvisionTenantRecord,
    ) -> AppResult<TenantProvisioningIdentity> {
        self.stage(Call::Identity, FailAt::Identity)?;
        Ok(TenantProvisioningIdentity {
            tenant_id: record.tenant_id.clone(),
            admin_role_id: 11,
            user_role_id: 12,
            provisioned_at: Utc::now(),
        })
    }

    async fn copy_tenant_authorization<'a>(
        &'a self,
        identity: &'a TenantProvisioningIdentity,
        template: TenantAuthorizationTemplate,
    ) -> AppResult<()> {
        self.stage(
            Call::Authorization(identity.clone(), template),
            FailAt::Authorization,
        )
    }

    async fn copy_tenant_base_catalogs<'a>(
        &'a self,
        identity: &'a TenantProvisioningIdentity,
        template: TenantBaseCatalogTemplate,
    ) -> AppResult<()> {
        self.stage(
            Call::BaseCatalogs(identity.clone(), template),
            FailAt::BaseCatalogs,
        )
    }

    async fn assign_initial_product<'a>(
        &'a self,
        _tenant_id: &'a str,
        _plan_version_id: i64,
        _changed_by: i64,
    ) -> AppResult<()> {
        self.state.push(Call::AssignProduct);
        Ok(())
    }

    async fn product_assignment<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> AppResult<Option<TenantProductAssignmentRecord>> {
        unreachable!("本测试不恢复产品快照")
    }

    async fn find_admin<'a>(
        &'a self,
        _tenant_id: &'a str,
        _username: &'a str,
    ) -> AppResult<Option<TenantAdminRecord>> {
        unreachable!("本测试不恢复管理员")
    }

    async fn save_tenant(&self, _tenant: TenantRecord) -> AppResult<TenantRecord> {
        unreachable!("本测试不保存租户")
    }

    async fn update_status<'a>(&'a self, _tenant_id: &'a str, _status: &'a str) -> AppResult<()> {
        self.state.push(Call::UpdateStatus);
        Ok(())
    }

    async fn create_pending<'a>(
        &'a self,
        _placement: &'a TenantProvisioningPlacement,
    ) -> AppResult<()> {
        self.state.push(Call::CreatePending);
        Ok(())
    }

    async fn create_or_resume_pending<'a>(
        &'a self,
        _placement: &'a TenantProvisioningPlacement,
    ) -> AppResult<()> {
        unreachable!("本测试不恢复放置记录")
    }

    async fn activate_placement<'a>(
        &'a self,
        _placement: &'a TenantProvisioningPlacement,
    ) -> AppResult<()> {
        self.state.push(Call::Activate);
        Ok(())
    }

    async fn fail_placement<'a>(
        &'a self,
        _placement: &'a TenantProvisioningPlacement,
    ) -> AppResult<()> {
        unreachable!("本测试不补偿放置记录")
    }
}

struct DummyProductRead;

#[async_trait]
impl ProductReadPort for DummyProductRead {
    async fn list_plans(&self) -> AppResult<Vec<ProductPlanRecord>> {
        unreachable!("本测试不读取产品")
    }

    async fn find_plan(&self, _plan_id: i64) -> AppResult<Option<ProductPlanRecord>> {
        unreachable!("本测试不读取产品")
    }

    async fn find_version(&self, _version_id: i64) -> AppResult<Option<ProductVersionSnapshot>> {
        unreachable!("本测试不读取产品")
    }

    async fn tenant_product<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> AppResult<Option<TenantProductSnapshot>> {
        unreachable!("本测试不读取产品")
    }
}

struct DummyProductWrite;

#[async_trait]
impl ryframe_application::ports::product::ProductWritePort for DummyProductWrite {
    async fn begin(
        &self,
    ) -> AppResult<Box<dyn ryframe_application::ports::product::ProductWriteTransaction>> {
        unreachable!("本测试不单独开启产品事务")
    }
}

struct DummyProvisioning;

#[async_trait]
impl TenantProvisioningPort for DummyProvisioning {
    fn prepare(
        &self,
        tenant_id: String,
        target_key: String,
        generation: i64,
        switch_token: String,
    ) -> AppResult<TenantProvisioningPlacement> {
        Ok(TenantProvisioningPlacement {
            tenant_id,
            target_key,
            generation,
            switch_token,
        })
    }

    async fn provision_fence(&self, _placement: &TenantProvisioningPlacement) -> AppResult<()> {
        Ok(())
    }
}

fn service(
    fail_at: Option<FailAt>,
    template: TenantProvisioningTemplate,
) -> (TenantService, Arc<FakeState>) {
    let state = Arc::new(FakeState {
        calls: Mutex::new(Vec::new()),
        fail_at,
    });
    let persistence = Arc::new(FakePersistence {
        state: Arc::clone(&state),
        template,
    });
    let product = Arc::new(ProductService::new(
        Arc::new(DummyProductRead),
        Arc::new(DummyProductWrite),
        AuthorizationCache::disabled(),
    ));
    (
        TenantService::new(
            persistence,
            AuthorizationCache::disabled(),
            product,
            Arc::new(DummyProvisioning),
        ),
        state,
    )
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 7,
        tenant_id: "system".into(),
        username: "platform-admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn params() -> CreateTenantParams {
    CreateTenantParams {
        idempotency_key: "tenant-provisioning-test".into(),
        tenant_id: "tenant-a".into(),
        name: "租户 A".into(),
        domain: None,
        expire_at: None,
        max_users: Some(100),
        max_roles: Some(20),
        max_storage_mb: Some(1024),
        max_requests_per_min: Some(1000),
        admin_username: "admin".into(),
        admin_password: "Valid!Pass123".into(),
        plan_version_id: 1,
        data_target_key: "primary".into(),
    }
}

async fn run_create(
    fail_at: Option<FailAt>,
    template: TenantProvisioningTemplate,
) -> (AppResult<TenantVo>, Arc<FakeState>) {
    let (service, state) = service(fail_at, template);
    let result = service.create(&actor(), params()).await;
    (result, state)
}

fn tenant(tenant_id: &str, status: &str) -> TenantRecord {
    let now = Utc::now();
    TenantRecord {
        id: 1,
        tenant_id: tenant_id.to_owned(),
        name: "租户 A".into(),
        domain: None,
        status: status.to_owned(),
        expire_at: None,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_min: 1000,
        session_version: 1,
        authorization_epoch: 1,
        runtime_epoch: 1,
        configuration_version: 0,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn every_provisioning_stage_failure_rolls_back_the_same_transaction() {
    for (stage, expected_prefix) in [
        (
            FailAt::Load,
            vec![Call::Begin, Call::Lock, Call::Product, Call::Load],
        ),
        (
            FailAt::Identity,
            vec![
                Call::Begin,
                Call::Lock,
                Call::Product,
                Call::Load,
                Call::Identity,
            ],
        ),
        (
            FailAt::Authorization,
            vec![
                Call::Begin,
                Call::Lock,
                Call::Product,
                Call::Load,
                Call::Identity,
            ],
        ),
        (
            FailAt::BaseCatalogs,
            vec![
                Call::Begin,
                Call::Lock,
                Call::Product,
                Call::Load,
                Call::Identity,
            ],
        ),
    ] {
        let (result, state) = run_create(
            Some(stage),
            TenantProvisioningTemplate {
                authorization: TenantAuthorizationTemplate::default(),
                base_catalogs: TenantBaseCatalogTemplate::default(),
            },
        )
        .await;
        let error = result.expect_err("注入阶段必须失败");
        assert_eq!(error.error_code(), ErrorCode::Internal);
        assert_eq!(error.message(), format!("模拟 {stage:?} 失败"));
        let calls = state.calls();
        assert_eq!(&calls[..expected_prefix.len()], expected_prefix);
        assert_eq!(calls.last(), Some(&Call::Rollback));
        assert!(!calls.iter().any(|call| matches!(call, Call::Commit(_))));
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, Call::AssignProduct | Call::CreatePending))
        );
        let failed_stage_index = calls.len() - 2;
        assert!(matches!(
            (&stage, &calls[failed_stage_index]),
            (FailAt::Load, Call::Load)
                | (FailAt::Identity, Call::Identity)
                | (FailAt::Authorization, Call::Authorization(_, _))
                | (FailAt::BaseCatalogs, Call::BaseCatalogs(_, _))
        ));
    }
}

#[tokio::test]
async fn provisioning_success_uses_four_stages_before_related_writes_and_commit() {
    let (result, state) = run_create(
        None,
        TenantProvisioningTemplate {
            authorization: TenantAuthorizationTemplate::default(),
            base_catalogs: TenantBaseCatalogTemplate::default(),
        },
    )
    .await;
    assert_eq!(result.unwrap().tenant_id, "tenant-a");
    assert!(matches!(
        state.calls().as_slice(),
        [
            Call::Begin,
            Call::Lock,
            Call::Product,
            Call::Load,
            Call::Identity,
            Call::Authorization(_, _),
            Call::BaseCatalogs(_, _),
            Call::AssignProduct,
            Call::CreatePending,
            Call::Commit(TransactionAuditMode::CurrentRequest),
            Call::Begin,
            Call::LockTenant,
            Call::Product,
            Call::SyncResources,
            Call::Commit(TransactionAuditMode::CurrentRequest),
            Call::Begin,
            Call::LockTenant,
            Call::Activate,
            Call::UpdateStatus,
            Call::Commit(TransactionAuditMode::Skip),
        ]
    ));
}

#[tokio::test]
async fn authorization_copy_receives_filtered_template_with_parent_closure_and_role_grants() {
    let (result, state) = run_create(None, policy_template()).await;
    result.expect("完整开通应成功");
    let calls = state.calls();
    let filtered = calls
        .iter()
        .find_map(|call| match call {
            Call::Authorization(_, template) => Some(template),
            _ => None,
        })
        .expect("授权复制阶段应收到过滤后的模板");

    let permissions = filtered
        .permissions
        .iter()
        .map(|permission| {
            (
                permission.code.as_str(),
                permission.assign_admin,
                permission.assign_user,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        permissions,
        [
            ("system:root", true, false),
            ("system:config-transfer:list", false, false),
            ("system:user:list", true, true),
            ("system:user:add", true, false),
            ("monitor:job:list", false, false),
        ]
    );
    assert_eq!(
        filtered
            .menus
            .iter()
            .map(|menu| menu.route_key.as_deref())
            .collect::<Vec<_>>(),
        [None, Some("system.config-transfer"), Some("system.users")]
    );
    let catalogs = calls
        .iter()
        .find_map(|call| match call {
            Call::BaseCatalogs(_, catalogs) => Some(catalogs),
            _ => None,
        })
        .expect("基础目录复制阶段应收到过滤后的模板");
    assert_eq!(catalogs.dictionary_data.len(), 1);
    assert_eq!(catalogs.dictionary_data[0].type_code, "active");
}

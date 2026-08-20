use crate::{AuthorizationMirrorTransaction, PersistenceFuture, ServiceAccountRecord};

pub trait ServiceAccountWriteTransaction: Send + Sync {
    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn account_code_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, bool>;

    fn department_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountRecord>>;

    fn insert_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord>;

    fn save_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord>;

    fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ServiceAccountWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ServiceAccountWriteTransaction>>;
}

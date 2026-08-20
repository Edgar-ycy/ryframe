use crate::PersistenceFuture;

#[derive(Debug)]
pub struct UserImportDepartmentRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub ancestors: String,
    pub status: String,
}

impl UserImportDepartmentRecord {
    const STATUS_NORMAL: &'static str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

pub trait UserImportDepartmentReadPort: Send + Sync {
    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<UserImportDepartmentRecord>>;
}

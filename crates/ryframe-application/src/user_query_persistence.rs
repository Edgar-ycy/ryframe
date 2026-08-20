use chrono::{DateTime, Utc};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

pub const USER_QUERY_STATUS_NORMAL: &str = "1";

#[derive(Clone, Debug)]
pub struct UserQueryRecord {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<i64>,
    pub dept_name: Option<String>,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl UserQueryRecord {
    pub fn is_enabled(&self) -> bool {
        self.status == USER_QUERY_STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct UserQueryRoleRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

#[derive(Debug)]
pub struct UserQueryDetailRecord {
    pub user: UserQueryRecord,
    pub roles: Vec<UserQueryRoleRecord>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UserQueryFilter<'a> {
    pub username: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub status: Option<&'a str>,
    pub dept_id: Option<i64>,
}

pub trait UserQueryReadPort: Send + Sync {
    fn export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<UserQueryRecord>>;

    fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        query: ValidatedPageQuery,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<UserQueryRecord>>;

    fn options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        scope: &'a DataScopeContext,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<UserQueryRecord>>;

    fn detail<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, Option<UserQueryDetailRecord>>;

    fn is_accessible<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, bool>;

    fn is_super_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, bool>;
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{USER_QUERY_STATUS_NORMAL, UserQueryRecord};

    fn user(status: &str) -> UserQueryRecord {
        UserQueryRecord {
            id: 1,
            username: "test".to_owned(),
            nickname: "测试".to_owned(),
            email: String::new(),
            phone: String::new(),
            avatar: None,
            status: status.to_owned(),
            dept_id: None,
            dept_name: None,
            remark: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn only_normal_status_is_enabled() {
        assert!(user(USER_QUERY_STATUS_NORMAL).is_enabled());
        assert!(!user("0").is_enabled());
        assert!(!user("2").is_enabled());
    }
}

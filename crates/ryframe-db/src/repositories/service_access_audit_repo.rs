use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ActiveModelTrait, ConnectionTrait};

use crate::entities::service_access_audit;

pub struct ServiceAccessAuditRepository;

impl ServiceAccessAuditRepository {
    /// 审计必须与 Agent 查询结果处于同一事务；写入失败时调用方必须回滚并拒绝返回数据。
    pub async fn insert<C>(
        &self,
        db: &C,
        audit: service_access_audit::Model,
    ) -> AppResult<service_access_audit::Model>
    where
        C: ConnectionTrait,
    {
        service_access_audit::ActiveModel::from(audit)
            .insert(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }
}

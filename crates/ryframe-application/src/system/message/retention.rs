use ryframe_kernel::AppResult;

use super::MessageService;

const RETENTION_BATCH_SIZE: u64 = 500;

impl MessageService {
    /// 删除已到期的消息及其级联收件箱记录。
    pub async fn delete_expired(&self) -> AppResult<u64> {
        self.ensure_enabled()?;
        let now = self.queue.database_now().await?;
        let mut deleted = 0_u64;
        loop {
            let transaction = self.persistence.begin().await?;
            let batch = transaction
                .delete_expired_batch(now, RETENTION_BATCH_SIZE)
                .await?;
            transaction.commit().await?;
            deleted = deleted.saturating_add(batch);
            if batch < RETENTION_BATCH_SIZE {
                return Ok(deleted);
            }
        }
    }
}

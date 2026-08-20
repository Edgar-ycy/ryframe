use super::*;

pub struct DataRetentionJobHandler {
    service: Arc<DataRetentionService>,
}

impl DataRetentionJobHandler {
    pub fn new(service: Arc<DataRetentionService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for DataRetentionJobHandler {
    fn job_type(&self) -> &'static str {
        DATA_RETENTION_JOB_TYPE
    }

    async fn handle(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        self.service.prepare_job(job).await?;
        self.service.execute_job(job).await
    }
}

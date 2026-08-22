/// Worker 中执行可恢复用户导入的处理器。
pub struct UserImportJobHandler {
    service: Arc<UserImportService>,
}

impl UserImportJobHandler {
    pub fn new(service: Arc<UserImportService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for UserImportJobHandler {
    fn job_type(&self) -> &'static str {
        USER_IMPORT_JOB_TYPE
    }

    async fn handle(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        self.service.execute_background_job(job.id).await
    }

    fn should_dead_letter(&self, error: &AppError) -> bool {
        is_terminal_import_error(error)
    }
}

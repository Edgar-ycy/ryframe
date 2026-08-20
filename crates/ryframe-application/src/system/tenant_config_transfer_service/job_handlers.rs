use super::*;

macro_rules! transfer_job_handler {
    ($name:ident, $job_type:expr, $method:ident) => {
        pub struct $name {
            service: Arc<TenantConfigTransferService>,
        }

        impl $name {
            pub fn new(service: Arc<TenantConfigTransferService>) -> Self {
                Self { service }
            }
        }

        #[async_trait]
        impl JobHandler for $name {
            fn job_type(&self) -> &'static str {
                $job_type
            }

            async fn handle(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
                self.service.$method(job).await
            }

            fn should_dead_letter(&self, error: &AppError) -> bool {
                matches!(
                    error,
                    AppError::Validation(_)
                        | AppError::Authorization(_)
                        | AppError::NotFound(_)
                        | AppError::Conflict(_)
                        | AppError::PayloadTooLarge(_)
                )
            }
        }
    };
}

transfer_job_handler!(
    TenantConfigExportJobHandler,
    TENANT_CONFIG_EXPORT_JOB_TYPE,
    execute_export
);
transfer_job_handler!(
    TenantConfigPreviewJobHandler,
    TENANT_CONFIG_PREVIEW_JOB_TYPE,
    execute_preview
);
transfer_job_handler!(
    TenantConfigApplyJobHandler,
    TENANT_CONFIG_APPLY_JOB_TYPE,
    execute_apply
);
transfer_job_handler!(
    TenantConfigRollbackJobHandler,
    TENANT_CONFIG_ROLLBACK_JOB_TYPE,
    execute_rollback
);

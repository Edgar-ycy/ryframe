use std::sync::Arc;

use ryframe_application::{ExportCleanupJobHandler, ExportJobHandler, JobHandler};

use super::super::JobWorkerDependencies;

pub(super) fn handlers(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(ExportJobHandler::new(dependencies.export.clone())),
        Arc::new(ExportCleanupJobHandler::new(dependencies.export.clone())),
    ]
}

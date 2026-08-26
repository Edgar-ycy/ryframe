use std::sync::Arc;

use ryframe_application::{JobHandler, system::operations::DataRetentionJobHandler};

use super::super::JobWorkerDependencies;

pub(super) fn handlers(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    vec![Arc::new(DataRetentionJobHandler::new(
        dependencies.data_retention.clone(),
    ))]
}

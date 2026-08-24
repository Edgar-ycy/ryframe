use std::sync::Arc;

use ryframe_application::{JobHandler, system::UserImportJobHandler};

use super::super::JobWorkerDependencies;

pub(super) fn handlers(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    vec![Arc::new(UserImportJobHandler::new(
        dependencies.user_import.clone(),
    ))]
}

use std::sync::Arc;

use ryframe_application::JobHandler;

use super::JobWorkerDependencies;

mod exports;
mod messages;
mod retention;
mod tenant;
mod users;

/// 按稳定领域顺序收集内置任务处理器。
pub(super) fn built_in(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    let mut handlers = Vec::new();
    handlers.extend(exports::handlers(dependencies));
    handlers.extend(messages::handlers(dependencies));
    handlers.extend(retention::handlers(dependencies));
    handlers.extend(tenant::handlers(dependencies));
    handlers.extend(users::handlers(dependencies));
    handlers
}

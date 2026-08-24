use std::sync::Arc;

use ryframe_application::{
    ScheduledJobTarget, maintenance_schedule_targets, message_schedule_targets,
};

pub(super) fn built_in(messaging_enabled: bool) -> Vec<Arc<dyn ScheduledJobTarget>> {
    let mut targets = maintenance_schedule_targets();
    targets.extend(message_schedule_targets(messaging_enabled));
    targets
}

use std::{path::PathBuf, process::Child, time::Duration};

use crate::workspace::root_dir;

use super::{build::cleanup_binaries, services::stop_all};

pub(super) const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const WATCH_DEBOUNCE: Duration = Duration::from_millis(350);
pub(super) const LOOP_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub(super) struct Binaries {
    pub(super) api: PathBuf,
    pub(super) worker: PathBuf,
}

pub(super) struct Services {
    pub(super) api: Child,
    pub(super) worker: Child,
    pub(super) binaries: Binaries,
}

pub(super) struct RunningProcesses {
    pub(super) services: Services,
    pub(super) vite: Child,
}

impl Drop for RunningProcesses {
    fn drop(&mut self) {
        let _ = stop_all(&mut self.services, &mut self.vite);
        let _ = cleanup_binaries(&root_dir(), &self.services.binaries);
    }
}

pub(super) enum BuildResult {
    Ready(Binaries),
    Failed,
    Cancelled,
}

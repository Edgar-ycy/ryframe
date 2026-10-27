//! 同进程一次性启动预留；调用方只能读取ID，不能指定或复制预留。
use super::{environment::Environment, operation, role::Role};
use crate::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/start_tests.rs"]
mod tests;

pub(crate) struct Reservation {
    pub(super) role: Role,
    pub(super) backend: PathBuf,
    pub(super) directory: PathBuf,
    id: String,
    environment: Environment,
    runtime: Value,
    source: String,
}

pub(crate) fn reserve(
    role: Role,
    backend: &Path,
    runtime_dir: &Path,
    environment_file: &Value,
) -> Result<Reservation> {
    if role == Role::Frontend {
        return Err("启动预留只允许API或Worker".into());
    }
    let backend = operation::canonical(backend)?;
    let directory = operation::canonical(runtime_dir)?;
    let environment = Environment::capture(environment_file)?;
    let runtime = super::runtime::load(role, &backend, &directory)?;
    let source = operation::source(&directory)?;
    let reservation = Reservation {
        role,
        backend,
        directory,
        id: uuid::Uuid::new_v4().simple().to_string(),
        environment,
        runtime,
        source,
    };
    reservation.verify()?;
    Ok(reservation)
}

impl Reservation {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn start(self, timeout_seconds: f64) -> Result<Value> {
        if !timeout_seconds.is_finite() || timeout_seconds <= 0.0 || timeout_seconds > 180.0 {
            return Err("启动预留超时必须在0到180秒之间".into());
        }
        self.verify()?;
        super::controller::reserved_start(&self, timeout_seconds)
    }

    pub(super) fn binding(&self) -> &Value {
        self.environment.binding()
    }

    pub(super) fn verify(&self) -> Result<()> {
        self.environment.check()?;
        if operation::canonical(&self.backend)? != self.backend
            || operation::canonical(&self.directory)? != self.directory
            || super::runtime::load(self.role, &self.backend, &self.directory)? != self.runtime
            || operation::source(&self.directory)? != self.source
        {
            return Err("启动预留后的根、运行合同或来源已变化；不启动、不重放".into());
        }
        self.environment.check()
    }
}

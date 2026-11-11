//! 旧复制目标启动前复核原初始化代次与完整复制完成证据。
use super::runtime_selection;
use crate::{
    Result,
    cli::CloneRole,
    recovery::clone::native::{
        json as document, resources::common, seed::source_registration::copied,
        target_verify::published,
    },
};
use serde_json::Value;
use std::path::Path;

pub(super) fn verify(
    backend: &Path,
    run: &Path,
    manifest: &Value,
    roles: &[CloneRole],
) -> Result<Value> {
    runtime_selection::validate_roles(roles)?;
    let binding = &manifest["initialized"];
    let initialized = common::bound_file(backend, binding, "目标启动初始化")?;
    let selected = published::observe_unlocked_history(backend, &initialized, |initial, _| {
        let plan = copied::require(backend, run, manifest)?;
        let pending = document::array(&plan["pending_target_actions"], "复制目标调度待办")?;
        if roles.contains(&CloneRole::Worker) && !pending.is_empty() {
            return Err("调度处置尚未完成，仅可启动 API，Worker 必须保持停止".into());
        }
        let selected = &initial["generation"]["selected"];
        document::object(selected, "目标初始化运行选择")?;
        Ok(selected.clone())
    })?;
    common::bound_file(backend, binding, "目标启动初始化")?;
    Ok(selected)
}

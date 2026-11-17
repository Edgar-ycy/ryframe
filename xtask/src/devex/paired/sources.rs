use std::path::Path;

use crate::Result;

use super::super::{
    execution::RunSession,
    metadata::{
        SourceFingerprints, collect_runtime_source_fingerprints, collect_source_fingerprints,
    },
    model::DevexRunOptions,
    preflight,
};

pub(super) fn verify_save_inputs(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
) -> Result<()> {
    if let Some(pairing) = &session.pairing
        && let Some(config) = &pairing.save_config
    {
        config.verify()?;
        if options.variant == "resource-manifest" {
            preflight::verify_save_resource_frontend(backend_root, frontend_root, pairing.arm)?;
        }
    }
    Ok(())
}

pub(super) fn source_fingerprints(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
) -> Result<SourceFingerprints> {
    if let Some(toolchain) = &session.common_toolchain {
        toolchain.check_files()?;
    }
    verify_save_inputs(backend_root, frontend_root, options, session)?;
    let observed = if session.source_fingerprints.frontend.is_some() {
        let runner = session
            .source_fingerprints
            .runner_frontend
            .as_ref()
            .map(|_| runner_frontend_root);
        collect_runtime_source_fingerprints(backend_root, frontend_root, runner)?
    } else {
        collect_source_fingerprints(backend_root, None)?
    };
    if observed != session.source_fingerprints {
        return Err(format!(
            "paired {} worktree 在测量期间发生变化，拒绝混入样本",
            session
                .pairing
                .as_ref()
                .map_or("unknown", |pairing| pairing.arm.as_str())
        )
        .into());
    }
    Ok(observed)
}

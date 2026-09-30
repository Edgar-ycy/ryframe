use std::path::Path;

use serde_json::json;

use crate::{
    Result,
    cli::{DatasetPrepareCommand, DatasetPrepareMode},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "tools/js/restore_reference_dataset.mjs";
const PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_DATASET_PREPARE";
const KIND: &str = "ryframe-xtask-recovery-dataset-prepare";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateDatasetPrepareInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &DatasetPrepareCommand, root: &Path) -> Result<()> {
    let DatasetPrepareCommand::Run(options) = command else {
        println!(
            "cargo xtask check recovery dataset-prepare --plan <绝对文件> \
             <--preflight <绝对文件>|--verify-existing <绝对文件> [--side source|target]> \
             --write"
        );
        return Ok(());
    };
    let invocation = private_invocation_at(options, root)?;
    run_with_env_removed(
        root,
        "node",
        &[invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

pub(crate) fn private_invocation_at(
    options: &crate::cli::DatasetPrepareOptions,
    root: &Path,
) -> Result<PrivateDatasetPrepareInvocation> {
    validate_local_test_path(&options.plan, root, LocalTestPathKind::ExistingFile)?;
    let (preflight, verify_existing, side) = match &options.mode {
        DatasetPrepareMode::Prepare { preflight } => {
            validate_local_test_path(preflight, root, LocalTestPathKind::ExistingFile)?;
            (Some(path_text(preflight)?), None, "source")
        }
        DatasetPrepareMode::VerifyExisting { dataset, side } => {
            validate_local_test_path(dataset, root, LocalTestPathKind::ExistingFile)?;
            (None, Some(path_text(dataset)?), side.as_str())
        }
    };
    let protocol = json!({
        "backend_dir": path_text(root)?,
        "format_version": 1,
        "kind": KIND,
        "plan": path_text(&options.plan)?,
        "preflight": preflight,
        "side": side,
        "verify_existing": verify_existing,
        "write": true,
    });
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.len() > 32 * 1024 || serialized.contains(['\n', '\r', '\0']) {
        return Err("dataset-prepare 私有协议过长或包含换行符/NUL".into());
    }
    Ok(PrivateDatasetPrepareInvocation {
        script: SCRIPT,
        protocol: serialized,
    })
}

fn path_text(value: &Path) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| "dataset-prepare 路径必须能表示为 UTF-8".into())
}

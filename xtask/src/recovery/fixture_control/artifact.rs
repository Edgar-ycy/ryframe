use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FixtureArtifactCommand, FixtureArtifactOptions},
    local_test_path::LocalTestPathKind,
};

use super::{insert_path, output_value, path_value};

pub(super) fn fields(command: &FixtureArtifactCommand, root: &Path) -> Result<Map<String, Value>> {
    let (options, snapshot) = match command {
        FixtureArtifactCommand::Help => return Err("artifact 帮助没有私有请求".into()),
        FixtureArtifactCommand::Snapshot(options) => (options, true),
        FixtureArtifactCommand::VerifyDeleted(options) => (options, false),
    };
    artifact_fields(options, root, snapshot)
}

fn artifact_fields(
    options: &FixtureArtifactOptions,
    root: &Path,
    snapshot: bool,
) -> Result<Map<String, Value>> {
    if options.job_id <= 0 {
        return Err("导出任务 ID 必须是正 i64".into());
    }
    let mut fields = Map::new();
    insert_path(
        &mut fields,
        "runtime_dir",
        &options.runtime_dir,
        root,
        LocalTestPathKind::ExistingDirectory,
    )?;
    fields.insert(
        "job_id".to_owned(),
        Value::String(options.job_id.to_string()),
    );
    let receipt = if snapshot {
        output_value(&options.receipt, root, "artifact 收据")?
    } else {
        path_value(
            &options.receipt,
            root,
            LocalTestPathKind::ExistingFile,
            "artifact 收据",
        )?
    };
    fields.insert("receipt".to_owned(), receipt);
    Ok(fields)
}

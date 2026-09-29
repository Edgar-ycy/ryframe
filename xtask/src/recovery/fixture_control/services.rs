use std::path::Path;

use serde_json::{Map, Value};

use crate::{Result, cli::FixtureServicesCommand, local_test_path::LocalTestPathKind};

use super::{insert_optional_path, insert_path};

pub(super) fn fields(command: &FixtureServicesCommand, root: &Path) -> Result<Map<String, Value>> {
    let FixtureServicesCommand::Run(options) = command else {
        return Err("services 帮助没有私有请求".into());
    };
    let mut fields = Map::new();
    insert_path(
        &mut fields,
        "review",
        &options.review,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    insert_path(
        &mut fields,
        "environment",
        &options.environment,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    insert_optional_path(
        &mut fields,
        "owner_binding",
        options.owner_binding.as_ref(),
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    Ok(fields)
}

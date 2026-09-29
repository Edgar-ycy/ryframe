use std::path::Path;

use serde_json::{Map, Value};

use crate::{Result, cli::FixtureRequestCommand, local_test_path::LocalTestPathKind};

use super::{insert_path, output_value};

pub(super) fn fields(command: &FixtureRequestCommand, root: &Path) -> Result<Map<String, Value>> {
    let FixtureRequestCommand::Publish(options) = command else {
        return Err("request 帮助没有私有请求".into());
    };
    let mut fields = Map::new();
    insert_path(
        &mut fields,
        "environment",
        &options.environment,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    insert_path(
        &mut fields,
        "service_run",
        &options.service_run,
        root,
        LocalTestPathKind::ExistingDirectory,
    )?;
    fields.insert("id".to_owned(), Value::String(options.id.clone()));
    fields.insert(
        "side".to_owned(),
        Value::String(options.side.as_str().to_owned()),
    );
    fields.insert(
        "output".to_owned(),
        output_value(&options.output.output, root, "fresh-target 请求输出")?,
    );
    Ok(fields)
}

use std::path::Path;

use serde_json::{Map, Value};

use crate::{Result, cli::FixtureReviewCommand, local_test_path::LocalTestPathKind};

use super::{insert_path, output_value, path_value};

pub(super) fn fields(command: &FixtureReviewCommand, root: &Path) -> Result<Map<String, Value>> {
    let FixtureReviewCommand::Renew(options) = command else {
        return Err("review 帮助没有私有请求".into());
    };
    let mut fields = Map::new();
    for (name, value) in [
        ("template", options.template.as_path()),
        ("fixture", options.fixture.as_path()),
    ] {
        insert_path(
            &mut fields,
            name,
            value,
            root,
            LocalTestPathKind::ExistingFile,
        )?;
    }
    fields.insert(
        "future_root".to_owned(),
        path_value(
            &options.future_root,
            root,
            LocalTestPathKind::NewDirectory,
            "新运行根目录",
        )?,
    );
    fields.insert("id".to_owned(), Value::String(options.id.clone()));
    for (name, value) in [
        ("api_port", options.api_port),
        ("worker_port", options.worker_port),
        ("frontend_port", options.frontend_port),
        ("rustfs_api_port", options.rustfs_api_port),
        ("rustfs_console_port", options.rustfs_console_port),
        ("redis_port", options.redis_port),
    ] {
        fields.insert(name.to_owned(), Value::from(value));
    }
    fields.insert(
        "output".to_owned(),
        output_value(&options.output, root, "审阅计划输出")?,
    );
    Ok(fields)
}

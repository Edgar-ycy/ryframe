use std::path::Path;

use serde_json::{Map, Value};

use crate::{Result, cli::FixtureSourcePairCommand};

use super::output_value;

pub(super) fn fields(
    command: &FixtureSourcePairCommand,
    root: &Path,
) -> Result<Map<String, Value>> {
    match command {
        FixtureSourcePairCommand::Help => Err("source-pair 帮助不生成私有协议".into()),
        FixtureSourcePairCommand::Publish(options) => Ok(Map::from_iter([(
            "output".to_owned(),
            output_value(&options.output, root, "source-pair 输出")?,
        )])),
    }
}

use crate::cli::{CliError, FixtureFileOutput, FixtureSourcePairCommand};

use super::{ParsedOptions, output_file, require_write};

pub(super) fn parse(args: &[String]) -> Result<FixtureSourcePairCommand, CliError> {
    if matches!(args, [value] if matches!(value.as_str(), "--help" | "-h")) {
        return Ok(FixtureSourcePairCommand::Help);
    }
    let options = ParsedOptions::parse(args, &["--output"])?;
    require_write(&options, "source-pair publish")?;
    Ok(FixtureSourcePairCommand::Publish(FixtureFileOutput {
        output: output_file(options.require("--output")?, "--output")?,
    }))
}

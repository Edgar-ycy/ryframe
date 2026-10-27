use crate::cli::{CliError, FixtureFileOutput, FixtureSourcePairCommand};

use super::{ParsedOptions, new_directory, output_file, require_write};

pub(super) fn parse(args: &[String]) -> Result<FixtureSourcePairCommand, CliError> {
    if matches!(args, [value] if matches!(value.as_str(), "--help" | "-h")) {
        return Ok(FixtureSourcePairCommand::Help);
    }
    if let Some((operation, rest)) = args.split_first()
        && operation == "publish-execution"
    {
        let options = ParsedOptions::parse(rest, &["--output-dir"])?;
        require_write(&options, "source-pair publish-execution")?;
        return Ok(FixtureSourcePairCommand::PublishExecution {
            output_dir: new_directory(options.require("--output-dir")?, "--output-dir")?,
        });
    }
    let options = ParsedOptions::parse(args, &["--output"])?;
    require_write(&options, "source-pair publish")?;
    Ok(FixtureSourcePairCommand::Publish(FixtureFileOutput {
        output: output_file(options.require("--output")?, "--output")?,
    }))
}

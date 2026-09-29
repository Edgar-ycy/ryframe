use crate::{
    cli::{CliError, FixtureFileOutput, FixtureRequestCommand, FixtureRequestOptions},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, output_file, parse_side, require_write, valid_identifier};

pub(super) fn parse(args: &[String]) -> Result<FixtureRequestCommand, CliError> {
    if matches!(args, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FixtureRequestCommand::Help);
    }
    let options = ParsedOptions::parse(
        args,
        &[
            "--environment",
            "--service-run",
            "--id",
            "--side",
            "--output",
        ],
    )?;
    require_write(&options, "request publish")?;
    Ok(FixtureRequestCommand::Publish(FixtureRequestOptions {
        environment: options.path("--environment", LocalTestPathKind::ExistingFile)?,
        service_run: options.path("--service-run", LocalTestPathKind::ExistingDirectory)?,
        id: valid_identifier(options.require("--id")?, "--id")?,
        side: parse_side(options.require("--side")?, true)?,
        output: FixtureFileOutput {
            output: output_file(options.require("--output")?, "--output")?,
        },
    }))
}

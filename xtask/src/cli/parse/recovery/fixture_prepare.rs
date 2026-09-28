use crate::cli::{CliError, FixtureExpectedSources, FixturePrepareCommand, FixturePrepareOptions};

use super::{
    fixture_control::{ParsedOptions, new_directory, require_write},
    valid_commit_sha,
};

pub(super) fn parse(args: &[String]) -> Result<FixturePrepareCommand, CliError> {
    if matches!(args, [value] if matches!(value.as_str(), "--help" | "-h")) {
        return Ok(FixturePrepareCommand::Help);
    }
    let options = ParsedOptions::parse(
        args,
        &[
            "--output-dir",
            "--expected-backend-sha",
            "--expected-frontend-sha",
        ],
    )?;
    require_write(&options, "prepare")?;
    let output_dir = new_directory(options.require("--output-dir")?, "--output-dir")?;
    let expected_sources = expected_sources(&options)?;
    Ok(FixturePrepareCommand::Run(FixturePrepareOptions {
        output_dir,
        expected_sources,
    }))
}

fn expected_sources(options: &ParsedOptions) -> Result<Option<FixtureExpectedSources>, CliError> {
    let backend = options.optional("--expected-backend-sha");
    let frontend = options.optional("--expected-frontend-sha");
    let Some((backend_sha, frontend_sha)) = backend.zip(frontend) else {
        if backend.is_some() || frontend.is_some() {
            return Err(CliError::new(
                "正式夹具必须同时指定 --expected-backend-sha 与 --expected-frontend-sha",
            ));
        }
        return Ok(None);
    };
    if !valid_commit_sha(backend_sha) || !valid_commit_sha(frontend_sha) {
        return Err(CliError::new(
            "正式夹具提交必须是非零的 40 位小写十六进制 SHA",
        ));
    }
    Ok(Some(FixtureExpectedSources {
        backend_sha: backend_sha.to_owned(),
        frontend_sha: frontend_sha.to_owned(),
    }))
}

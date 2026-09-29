use std::path::Path;

use crate::{
    cli::{CliError, FixtureDatasetCommand, FixtureDatasetInputs},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, new_directory, output_file, parse_side, require_write};

const PLAN_OPTIONS: &[&str] = &[
    "--environment",
    "--runtime",
    "--work-dir",
    "--output",
    "--side",
];
const PREPARE_OPTIONS: &[&str] = &["--environment", "--runtime", "--plan", "--side"];

pub(super) fn parse(args: &[String]) -> Result<FixtureDatasetCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture dataset 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureDatasetCommand::Help);
    }
    if is_help(values) && matches!(operation.as_str(), "plan" | "prepare") {
        return Ok(FixtureDatasetCommand::Help);
    }
    match operation.as_str() {
        "plan" => parse_plan(values),
        "prepare" => parse_prepare(values),
        _ => Err(CliError::new(format!(
            "未知 fixture dataset 操作：{operation}"
        ))),
    }
}

fn is_help(values: &[String]) -> bool {
    matches!(values, [help] if matches!(help.as_str(), "--help" | "-h"))
}

fn parse_plan(args: &[String]) -> Result<FixtureDatasetCommand, CliError> {
    let options = ParsedOptions::parse(args, PLAN_OPTIONS)?;
    require_write(&options, "dataset plan")?;
    let inputs = inputs(&options)?;
    let work_dir = new_directory(options.require("--work-dir")?, "--work-dir")?;
    let output = output_file(options.require("--output")?, "--output")?;
    require_runtime_child(&inputs.runtime, &work_dir, "--work-dir")?;
    require_runtime_child(&inputs.runtime, &output, "--output")?;
    require_distinct(&work_dir, &output)?;
    Ok(FixtureDatasetCommand::Plan {
        inputs,
        work_dir,
        output,
    })
}

fn parse_prepare(args: &[String]) -> Result<FixtureDatasetCommand, CliError> {
    let options = ParsedOptions::parse(args, PREPARE_OPTIONS)?;
    require_write(&options, "dataset prepare")?;
    let inputs = inputs(&options)?;
    let plan = options.path("--plan", LocalTestPathKind::ExistingFile)?;
    require_runtime_child(&inputs.runtime, &plan, "--plan")?;
    Ok(FixtureDatasetCommand::Prepare { inputs, plan })
}

fn inputs(options: &ParsedOptions) -> Result<FixtureDatasetInputs, CliError> {
    Ok(FixtureDatasetInputs {
        environment: options.path("--environment", LocalTestPathKind::ExistingFile)?,
        runtime: options.path("--runtime", LocalTestPathKind::ExistingDirectory)?,
        side: parse_side(options.require("--side")?, false)?,
    })
}

fn require_runtime_child(runtime: &Path, value: &Path, label: &str) -> Result<(), CliError> {
    if value.parent() == Some(runtime) {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "{label} 必须是 --runtime 的直接子项"
        )))
    }
}

fn require_distinct(work_dir: &Path, output: &Path) -> Result<(), CliError> {
    if work_dir == output {
        Err(CliError::new(
            "--work-dir 与 --output 必须是两个不同的直接子项",
        ))
    } else {
        Ok(())
    }
}

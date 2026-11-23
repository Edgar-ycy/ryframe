use crate::{
    cli::{
        CliError, FixtureSuccessorArm, FixtureSuccessorArmInput, FixtureSuccessorCommand,
        FixtureSuccessorGeneration, FixtureSuccessorRelationship,
    },
    local_test_path::LocalTestPathKind,
};

use super::super::runtime_values::external_directory;
use super::{
    ParsedOptions, new_directory, output_file, parse_side, reject_write, require_write,
    valid_copy_name,
};

pub(super) fn parse(args: &[String]) -> Result<FixtureSuccessorCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture successor 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureSuccessorCommand::Help);
    }
    if matches!(values, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FixtureSuccessorCommand::Help);
    }
    match operation.as_str() {
        "relationship" => relationship(values),
        "generation-request" => generation(values),
        "arm-request" => arm(values),
        "arm-input" => arm_input(values),
        _ => Err(CliError::new(format!(
            "未知 fixture successor 操作：{operation}"
        ))),
    }
}

fn relationship(args: &[String]) -> Result<FixtureSuccessorCommand, CliError> {
    let options = ParsedOptions::parse(
        args,
        &[
            "--source-result",
            "--predecessor-review",
            "--predecessor-request",
            "--successor-review",
            "--seed-request",
            "--base-request",
            "--candidate-request",
            "--id",
            "--output",
        ],
    )?;
    require_write(&options, "successor relationship")?;
    Ok(FixtureSuccessorCommand::Relationship(
        FixtureSuccessorRelationship {
            source_result: input(&options, "--source-result")?,
            predecessor_review: input(&options, "--predecessor-review")?,
            predecessor_request: input(&options, "--predecessor-request")?,
            successor_review: input(&options, "--successor-review")?,
            seed_request: input(&options, "--seed-request")?,
            base_request: input(&options, "--base-request")?,
            candidate_request: input(&options, "--candidate-request")?,
            id: valid_copy_name(options.require("--id")?, "--id")?,
            output: output_file(options.require("--output")?, "--output")?,
        },
    ))
}

fn generation(args: &[String]) -> Result<FixtureSuccessorCommand, CliError> {
    let options = ParsedOptions::parse(
        args,
        &[
            "--successor",
            "--source-backend",
            "--expected-head",
            "--backend-build",
            "--maintenance-build",
            "--source-environment",
            "--config-dir",
            "--execution-source",
            "--id",
            "--adapter-contract",
            "--product-backend",
            "--output",
        ],
    )?;
    let output = paired_output(&options, "successor generation-request")?;
    let expected_head = options.require("--expected-head")?;
    if expected_head.len() != 40 || !expected_head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::new(
            "--expected-head 必须是 40 位十六进制 Git SHA",
        ));
    }
    let adapter_contract = options.optional("--adapter-contract");
    let product_backend = options.optional("--product-backend");
    if adapter_contract.is_some() != product_backend.is_some() {
        return Err(CliError::new(
            "--adapter-contract 与 --product-backend 必须同时指定",
        ));
    }
    if adapter_contract.is_some_and(|value| value != "legacy-stable-readiness-b0-v1") {
        return Err(CliError::new("--adapter-contract 不是已登记的 B0 适配关系"));
    }
    Ok(FixtureSuccessorCommand::GenerationRequest(
        FixtureSuccessorGeneration {
            successor: input(&options, "--successor")?,
            source_backend: external_directory(
                options.require("--source-backend")?,
                "--source-backend",
            )?,
            expected_head: expected_head.to_ascii_lowercase(),
            backend_build: input(&options, "--backend-build")?,
            maintenance_build: input(&options, "--maintenance-build")?,
            source_environment: input(&options, "--source-environment")?,
            config_dir: external_directory(options.require("--config-dir")?, "--config-dir")?,
            execution_source: options
                .optional("--execution-source")
                .map(|_| input(&options, "--execution-source"))
                .transpose()?,
            id: valid_copy_name(options.require("--id")?, "--id")?,
            adapter_contract: adapter_contract.map(ToOwned::to_owned),
            product_backend: product_backend
                .map(|value| external_directory(value, "--product-backend"))
                .transpose()?,
            output,
        },
    ))
}

fn arm(args: &[String]) -> Result<FixtureSuccessorCommand, CliError> {
    let (options, independent_b1) = ParsedOptions::parse_with_switch(
        args,
        &[
            "--successor",
            "--source-export-result",
            "--workspace",
            "--id",
            "--side",
            "--copy-directory",
            "--output",
        ],
        Some("--independent-b1"),
    )?;
    if independent_b1 && options.require("--side")? != "candidate" {
        return Err(CliError::new(
            "--independent-b1 必须与 --side candidate 一起使用",
        ));
    }
    let output = paired_output(&options, "successor arm-request")?;
    Ok(FixtureSuccessorCommand::ArmRequest(FixtureSuccessorArm {
        successor: input(&options, "--successor")?,
        source_export_result: input(&options, "--source-export-result")?,
        workspace: options.path("--workspace", LocalTestPathKind::ExistingDirectory)?,
        id: valid_copy_name(options.require("--id")?, "--id")?,
        side: parse_side(options.require("--side")?, false)?,
        independent_b1,
        copy_directory: new_directory(options.require("--copy-directory")?, "--copy-directory")?,
        output,
    }))
}

fn paired_output(
    options: &ParsedOptions,
    operation: &str,
) -> Result<Option<std::path::PathBuf>, CliError> {
    match (options.optional("--output"), options.write) {
        (None, false) => {
            reject_write(options, operation)?;
            Ok(None)
        }
        (Some(value), true) => output_file(value, "--output").map(Some),
        _ => Err(CliError::new(format!(
            "{operation} 写入必须同时指定 --output 与 --write；默认预览不落盘"
        ))),
    }
}

fn arm_input(args: &[String]) -> Result<FixtureSuccessorCommand, CliError> {
    let options = ParsedOptions::parse(args, &["--request", "--run-dir"])?;
    require_write(&options, "successor arm-input")?;
    Ok(FixtureSuccessorCommand::ArmInput(
        FixtureSuccessorArmInput {
            request: input(&options, "--request")?,
            run_dir: new_directory(options.require("--run-dir")?, "--run-dir")?,
        },
    ))
}

fn input(options: &ParsedOptions, name: &str) -> Result<std::path::PathBuf, CliError> {
    options.path(name, LocalTestPathKind::ExistingFile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_b1_arm_input_requires_explicit_write_and_a_new_run_directory() {
        let local = crate::workspace::root_dir().join(".local-tests");
        std::fs::create_dir_all(&local).unwrap();
        let fixture = tempfile::tempdir_in(local).unwrap();
        let request = fixture.path().join("request.json");
        let directory = fixture.path().join("new-run");
        std::fs::write(&request, b"{}").unwrap();
        let mut args = [
            "arm-input",
            "--request",
            request.to_str().unwrap(),
            "--run-dir",
            directory.to_str().unwrap(),
        ]
        .map(str::to_owned)
        .to_vec();
        assert!(parse(&args).is_err());
        args.push("--write".into());
        let parsed = parse(&args).unwrap();
        assert!(parsed.writes());
        assert_eq!(parsed.operation(), "arm-input");
        assert_eq!(
            parsed,
            FixtureSuccessorCommand::ArmInput(FixtureSuccessorArmInput {
                request: request.clone(),
                run_dir: directory.clone(),
            })
        );
        assert!(!directory.exists());
        for extra in [
            vec![
                "--run-dir".to_owned(),
                directory.to_string_lossy().into_owned(),
            ],
            vec!["--side".to_owned(), "candidate".to_owned()],
            vec!["--write".to_owned()],
        ] {
            assert!(parse(&[args.clone(), extra].concat()).is_err());
        }
        std::fs::create_dir(&directory).unwrap();
        assert!(parse(&args).is_err());
        assert_eq!(std::fs::read(&request).unwrap(), b"{}");
        assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
    }
}

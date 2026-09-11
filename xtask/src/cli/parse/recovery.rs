use super::super::model::{CliError, RecoveryCommand};

pub(super) fn parse_recovery(args: &[String]) -> Result<RecoveryCommand, CliError> {
    let Some((stage, rest)) = args.split_first() else {
        return Err(CliError::new("check recovery 缺少明确阶段"));
    };
    match stage.as_str() {
        "plan" | "check-dataset" | "check-existing" | "dataset" | "backup" | "restore" | "copy"
        | "damage" => Ok(RecoveryCommand::Reference(args.to_vec())),
        "runtime" => parse_recovery_operation("runtime", rest, &["build", "bind", "verify"])
            .map(RecoveryCommand::Runtime),
        "source" => parse_recovery_operation(
            "source",
            rest,
            &[
                "verify",
                "quiesce",
                "comparison-capture",
                "comparison-verify",
            ],
        )
        .map(RecoveryCommand::Source),
        "clone" => parse_recovery_operation(
            "clone",
            rest,
            &[
                "plan",
                "verify",
                "init",
                "status",
                "stage",
                "runtime",
                "recover",
                "recover-copy",
                "bridge",
                "post-copy",
                "seed-runtime",
                "storage",
                "cache",
                "maintenance",
            ],
        )
        .map(RecoveryCommand::Clone),
        "fresh-target" => Ok(RecoveryCommand::FreshTarget(rest.to_vec())),
        "fixture" => parse_fixture(rest).map(RecoveryCommand::Fixture),
        "dataset-prepare" => Ok(RecoveryCommand::DatasetPrepare(rest.to_vec())),
        _ => Err(CliError::new(format!("未知 recovery 阶段：{stage}"))),
    }
}

fn parse_fixture(args: &[String]) -> Result<Vec<String>, CliError> {
    match args {
        [option, ..] if option.starts_with("--") => Ok(args.to_vec()),
        [operation, ..]
            if [
                "environment",
                "review",
                "request",
                "successor",
                "artifact",
                "retention",
                "services",
                "source-pair",
                "runtime",
                "dataset",
            ]
            .contains(&operation.as_str()) =>
        {
            Ok(args.to_vec())
        }
        _ => Err(CliError::new(
            "用法：cargo xtask check recovery fixture --output-dir <目录> --write，或 fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...",
        )),
    }
}

fn parse_recovery_operation(
    stage: &str,
    args: &[String],
    operations: &[&str],
) -> Result<Vec<String>, CliError> {
    let Some(operation) = args.first() else {
        return Err(CliError::new(format!(
            "check recovery {stage} 缺少明确子操作"
        )));
    };
    if !operations.contains(&operation.as_str()) {
        return Err(CliError::new(format!(
            "未知 recovery {stage} 子操作：{operation}"
        )));
    }
    Ok(args.to_vec())
}

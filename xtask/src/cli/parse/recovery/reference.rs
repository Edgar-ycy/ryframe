use std::path::PathBuf;

use crate::{
    cli::{
        CliError, ExistingReferenceSide, RecoveryReferenceCommand, ReferencePlanAction,
        ReferencePlanOptions, ReferenceTargetInputs,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

#[derive(Default)]
struct Options {
    plan: Option<PathBuf>,
    inventory: Option<PathBuf>,
    source_generation: Option<PathBuf>,
    source_export_result: Option<PathBuf>,
    backup_root: Option<PathBuf>,
    record: Option<PathBuf>,
    runtime_registration: Option<PathBuf>,
    copy_id: Option<String>,
    artifact: Option<String>,
    side: Option<ExistingReferenceSide>,
    backup_receipt: Option<PathBuf>,
    comparison_sources: Option<PathBuf>,
    arm_input: Option<PathBuf>,
    fresh_target_verify: Option<PathBuf>,
    product_plan: Option<PathBuf>,
    target_plan: Option<PathBuf>,
    output: Option<PathBuf>,
    missing: bool,
    write: bool,
}

pub(super) fn parse(args: &[String]) -> Result<RecoveryReferenceCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("check recovery 缺少明确阶段"));
    };
    if help_request(operation, values) {
        return Ok(RecoveryReferenceCommand::Help);
    }
    if ![
        "plan",
        "check-dataset",
        "check-existing",
        "dataset",
        "backup",
        "restore",
        "copy",
        "damage",
    ]
    .contains(&operation.as_str())
    {
        return Err(CliError::new(format!("未知 recovery 阶段：{operation}")));
    }
    let options = parse_options(values)?;
    build_command(operation, options)
}

fn help_request(operation: &str, values: &[String]) -> bool {
    (matches!(operation, "--help" | "-h") && values.is_empty())
        || (values.len() == 1 && matches!(values[0].as_str(), "--help" | "-h"))
}

fn parse_options(args: &[String]) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if matches!(name, "--write" | "--missing") {
            set_flag(&mut options, name)?;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{name} 缺少有效取值")))?;
        set_value(&mut options, name, value)?;
        index += 2;
    }
    Ok(options)
}

fn set_flag(options: &mut Options, name: &str) -> Result<(), CliError> {
    let target = match name {
        "--write" => &mut options.write,
        "--missing" => &mut options.missing,
        _ => unreachable!("flag 已由调用方筛选"),
    };
    if *target {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = true;
    Ok(())
}

fn set_value(options: &mut Options, name: &str, value: &str) -> Result<(), CliError> {
    match name {
        "--plan" => set_path(&mut options.plan, name, value),
        "--inventory" => set_path(&mut options.inventory, name, value),
        "--source-generation" => set_path(&mut options.source_generation, name, value),
        "--source-export-result" => set_path(&mut options.source_export_result, name, value),
        "--backup-root" => set_path(&mut options.backup_root, name, value),
        "--record" => set_path(&mut options.record, name, value),
        "--runtime-registration" => set_path(&mut options.runtime_registration, name, value),
        "--copy-id" => set_string(&mut options.copy_id, name, value),
        "--artifact" => set_string(&mut options.artifact, name, value),
        "--side" => set_side(&mut options.side, name, value),
        "--backup-receipt" => set_path(&mut options.backup_receipt, name, value),
        "--comparison-sources" => set_path(&mut options.comparison_sources, name, value),
        "--arm-input" => set_path(&mut options.arm_input, name, value),
        "--fresh-target-verify" => set_path(&mut options.fresh_target_verify, name, value),
        "--product-plan" => set_path(&mut options.product_plan, name, value),
        "--target-plan" => set_path(&mut options.target_plan, name, value),
        "--output" => set_path(&mut options.output, name, value),
        _ => Err(CliError::new(format!(
            "未知 recovery reference 参数：{name}"
        ))),
    }
}

fn build_command(
    operation: &str,
    mut options: Options,
) -> Result<RecoveryReferenceCommand, CliError> {
    let plan = required_path(&mut options.plan, "--plan", LocalTestPathKind::ExistingFile)?;
    match operation {
        "plan" => build_plan(plan, options),
        "check-dataset" => build_check_dataset(plan, options),
        "check-existing" => build_check_existing(plan, options),
        "dataset" => build_dataset(plan, options),
        "backup" => build_backup(plan, options),
        "restore" => build_restore(plan, options),
        "copy" => build_copy(plan, options),
        "damage" => build_damage(plan, options),
        _ => unreachable!("operation 已由调用方核验"),
    }
}

fn build_plan(plan: PathBuf, mut options: Options) -> Result<RecoveryReferenceCommand, CliError> {
    if options.missing {
        return Err(CliError::new("--missing 只用于 damage"));
    }
    let input_count = [
        options.backup_receipt.is_some(),
        options.comparison_sources.is_some(),
        options.arm_input.is_some(),
        options.fresh_target_verify.is_some(),
        options.product_plan.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    let action = if let Some(target_plan) = options.target_plan.take() {
        if input_count != 0 || options.output.is_some() || options.write {
            return Err(CliError::new(
                "plan 复核 --target-plan 不能与创建输入、--output 或 --write 同时使用",
            ));
        }
        ReferencePlanAction::Verify {
            target_plan: local_path(target_plan, LocalTestPathKind::ExistingFile, "目标计划")?,
        }
    } else if input_count == 0 {
        if options.output.is_some() || options.write {
            return Err(CliError::new(
                "plan 摘要是只读操作，不接受 --output 或 --write",
            ));
        }
        ReferencePlanAction::Summary
    } else {
        if input_count != 5 {
            return Err(CliError::new("目标计划必须完整提供五份创建输入"));
        }
        let inputs = take_target_inputs(&mut options)?;
        match (options.output.take(), options.write) {
            (None, false) => ReferencePlanAction::Preview { inputs },
            (Some(output), true) => ReferencePlanAction::Publish {
                inputs,
                output: output_path(output, "目标计划输出")?,
            },
            _ => {
                return Err(CliError::new(
                    "目标计划发布必须同时指定 --output 与 --write；预览不接受其中任一项",
                ));
            }
        }
    };
    options.write = false;
    ensure_empty(&options, "plan")?;
    Ok(RecoveryReferenceCommand::Plan(ReferencePlanOptions {
        plan,
        action,
    }))
}

fn take_target_inputs(options: &mut Options) -> Result<ReferenceTargetInputs, CliError> {
    Ok(ReferenceTargetInputs {
        backup_receipt: required_path(
            &mut options.backup_receipt,
            "--backup-receipt",
            LocalTestPathKind::ExistingFile,
        )?,
        comparison_sources: required_path(
            &mut options.comparison_sources,
            "--comparison-sources",
            LocalTestPathKind::ExistingFile,
        )?,
        arm_input: required_path(
            &mut options.arm_input,
            "--arm-input",
            LocalTestPathKind::ExistingFile,
        )?,
        fresh_target_verify: required_path(
            &mut options.fresh_target_verify,
            "--fresh-target-verify",
            LocalTestPathKind::ExistingFile,
        )?,
        product_plan: required_path(
            &mut options.product_plan,
            "--product-plan",
            LocalTestPathKind::ExistingFile,
        )?,
    })
}

fn build_check_dataset(
    plan: PathBuf,
    options: Options,
) -> Result<RecoveryReferenceCommand, CliError> {
    reject_flags(&options, false, false, "check-dataset")?;
    ensure_empty(&options, "check-dataset")?;
    Ok(RecoveryReferenceCommand::CheckDataset { plan })
}

fn build_check_existing(
    plan: PathBuf,
    mut options: Options,
) -> Result<RecoveryReferenceCommand, CliError> {
    reject_flags(&options, false, false, "check-existing")?;
    let side = options.side.take().unwrap_or(ExistingReferenceSide::Target);
    ensure_empty(&options, "check-existing")?;
    Ok(RecoveryReferenceCommand::CheckExisting { plan, side })
}

fn build_dataset(
    plan: PathBuf,
    mut options: Options,
) -> Result<RecoveryReferenceCommand, CliError> {
    require_write(&mut options, "dataset")?;
    ensure_empty(&options, "dataset")?;
    Ok(RecoveryReferenceCommand::Dataset { plan })
}

fn build_backup(plan: PathBuf, mut options: Options) -> Result<RecoveryReferenceCommand, CliError> {
    require_write(&mut options, "backup")?;
    let inventory = required_path(
        &mut options.inventory,
        "--inventory",
        LocalTestPathKind::ExistingFile,
    )?;
    let source_generation = required_path(
        &mut options.source_generation,
        "--source-generation",
        LocalTestPathKind::ExistingFile,
    )?;
    let source_export_result = required_path(
        &mut options.source_export_result,
        "--source-export-result",
        LocalTestPathKind::ExistingFile,
    )?;
    ensure_empty(&options, "backup")?;
    Ok(RecoveryReferenceCommand::Backup {
        plan,
        inventory,
        source_generation,
        source_export_result,
    })
}

fn build_restore(
    plan: PathBuf,
    mut options: Options,
) -> Result<RecoveryReferenceCommand, CliError> {
    require_write(&mut options, "restore")?;
    let backup_root = required_path(
        &mut options.backup_root,
        "--backup-root",
        LocalTestPathKind::ExistingDirectory,
    )?;
    let record = required_path(
        &mut options.record,
        "--record",
        LocalTestPathKind::ExistingFile,
    )?;
    let target_plan = required_path(
        &mut options.target_plan,
        "--target-plan",
        LocalTestPathKind::ExistingFile,
    )?;
    let runtime_registration = required_path(
        &mut options.runtime_registration,
        "--runtime-registration",
        LocalTestPathKind::ExistingFile,
    )?;
    ensure_empty(&options, "restore")?;
    Ok(RecoveryReferenceCommand::Restore {
        plan,
        backup_root,
        record,
        target_plan,
        runtime_registration,
    })
}

fn build_copy(plan: PathBuf, mut options: Options) -> Result<RecoveryReferenceCommand, CliError> {
    require_write(&mut options, "copy")?;
    let backup_root = required_path(
        &mut options.backup_root,
        "--backup-root",
        LocalTestPathKind::ExistingDirectory,
    )?;
    let copy_id = required_string(&mut options.copy_id, "--copy-id")?;
    validate_identifier(&copy_id, "--copy-id")?;
    ensure_empty(&options, "copy")?;
    Ok(RecoveryReferenceCommand::Copy {
        plan,
        backup_root,
        copy_id,
    })
}

fn build_damage(plan: PathBuf, mut options: Options) -> Result<RecoveryReferenceCommand, CliError> {
    require_write(&mut options, "damage")?;
    let backup_root = required_path(
        &mut options.backup_root,
        "--backup-root",
        LocalTestPathKind::ExistingDirectory,
    )?;
    let artifact = required_string(&mut options.artifact, "--artifact")?;
    validate_artifact(&artifact)?;
    let missing = options.missing;
    options.missing = false;
    ensure_empty(&options, "damage")?;
    Ok(RecoveryReferenceCommand::Damage {
        plan,
        backup_root,
        artifact,
        missing,
    })
}

fn require_write(options: &mut Options, operation: &str) -> Result<(), CliError> {
    if !options.write {
        return Err(CliError::new(format!(
            "recovery {operation} 会修改登记资源或发布阶段证据，必须显式传入 --write"
        )));
    }
    options.write = false;
    Ok(())
}

fn reject_flags(
    options: &Options,
    write: bool,
    missing: bool,
    operation: &str,
) -> Result<(), CliError> {
    if options.write != write {
        return Err(CliError::new(format!(
            "recovery {operation} 是只读操作，不接受 --write"
        )));
    }
    if options.missing != missing {
        return Err(CliError::new("--missing 只用于 damage"));
    }
    Ok(())
}

fn ensure_empty(options: &Options, operation: &str) -> Result<(), CliError> {
    let present = [
        ("--inventory", options.inventory.is_some()),
        ("--source-generation", options.source_generation.is_some()),
        (
            "--source-export-result",
            options.source_export_result.is_some(),
        ),
        ("--backup-root", options.backup_root.is_some()),
        ("--record", options.record.is_some()),
        (
            "--runtime-registration",
            options.runtime_registration.is_some(),
        ),
        ("--copy-id", options.copy_id.is_some()),
        ("--artifact", options.artifact.is_some()),
        ("--side", options.side.is_some()),
        ("--backup-receipt", options.backup_receipt.is_some()),
        ("--comparison-sources", options.comparison_sources.is_some()),
        ("--arm-input", options.arm_input.is_some()),
        (
            "--fresh-target-verify",
            options.fresh_target_verify.is_some(),
        ),
        ("--product-plan", options.product_plan.is_some()),
        ("--target-plan", options.target_plan.is_some()),
        ("--output", options.output.is_some()),
        ("--missing", options.missing),
        ("--write", options.write),
    ];
    if let Some((name, _)) = present.into_iter().find(|(_, value)| *value) {
        Err(CliError::new(format!("{name} 不属于 recovery {operation}")))
    } else {
        Ok(())
    }
}

fn required_path(
    target: &mut Option<PathBuf>,
    name: &str,
    kind: LocalTestPathKind,
) -> Result<PathBuf, CliError> {
    let value = target
        .take()
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {name}")))?;
    local_path(value, kind, name)
}

fn required_string(target: &mut Option<String>, name: &str) -> Result<String, CliError> {
    target
        .take()
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {name}")))
}

fn local_path(value: PathBuf, kind: LocalTestPathKind, label: &str) -> Result<PathBuf, CliError> {
    let root = root_dir();
    let value = if value.is_absolute() {
        value
    } else {
        root.join(value)
    };
    validate_local_test_path(&value, &root, kind)
        .map_err(|error| CliError::new(format!("{label} 无效：{error}")))?;
    Ok(value)
}

fn output_path(value: PathBuf, label: &str) -> Result<PathBuf, CliError> {
    let value = local_path(value, LocalTestPathKind::OutputFile, label)?;
    let parent = value
        .parent()
        .ok_or_else(|| CliError::new(format!("{label} 缺少父目录")))?;
    if !parent.is_dir() {
        return Err(CliError::new(format!("{label} 的父目录必须已经存在")));
    }
    Ok(value)
}

fn set_path(target: &mut Option<PathBuf>, name: &str, value: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(PathBuf::from(value));
    Ok(())
}

fn set_string(target: &mut Option<String>, name: &str, value: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(value.to_owned());
    Ok(())
}

fn set_side(
    target: &mut Option<ExistingReferenceSide>,
    name: &str,
    value: &str,
) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(match value {
        "source" => ExistingReferenceSide::Source,
        "target" => ExistingReferenceSide::Target,
        _ => return Err(CliError::new("--side 只允许 source 或 target")),
    });
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), CliError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && !value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(CliError::new(format!(
            "{label} 必须以小写字母或数字开头，只包含小写字母、数字、下划线或连字符，最长 64 字节"
        )));
    }
    Ok(())
}

fn validate_artifact(value: &str) -> Result<(), CliError> {
    if value.is_empty()
        || value.contains(['\\', ':', '\n', '\r', '\0'])
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || value.chars().any(|character| character.is_control())
    {
        Err(CliError::new(
            "--artifact 必须是备份根内不含跳转、反斜杠或控制字符的相对路径",
        ))
    } else {
        Ok(())
    }
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r', '\0'])
}

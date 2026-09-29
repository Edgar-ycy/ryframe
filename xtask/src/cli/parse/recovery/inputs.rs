use std::path::PathBuf;

use chrono::DateTime;

use crate::{
    cli::{
        BindingsInputOptions, CliError, ProductInputOptions, RecoveryInputsCommand,
        ReferenceInputOptions, RestoreInputPublication, RestoreInputSide,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

#[derive(Default)]
struct Options {
    arm_input: Option<PathBuf>,
    fresh_target_verify: Option<PathBuf>,
    side: Option<RestoreInputSide>,
    id: Option<String>,
    work_dir: Option<PathBuf>,
    reference_plan: Option<PathBuf>,
    backup_receipt: Option<PathBuf>,
    comparison_sources: Option<PathBuf>,
    fault_at: Option<String>,
    target_plan: Option<PathBuf>,
    record: Option<PathBuf>,
    output: Option<PathBuf>,
    write: bool,
}

pub(super) fn parse(args: &[String]) -> Result<RecoveryInputsCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("check recovery inputs 缺少明确子操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(RecoveryInputsCommand::Help);
    }
    if !["reference", "product", "bindings"].contains(&operation.as_str()) {
        return Err(CliError::new(format!(
            "未知 recovery inputs 子操作：{operation}"
        )));
    }
    if values.len() == 1 && matches!(values[0].as_str(), "--help" | "-h") {
        return Ok(RecoveryInputsCommand::Help);
    }
    let options = parse_options(values)?;
    match operation.as_str() {
        "reference" => build_reference(options),
        "product" => build_product(options),
        "bindings" => build_bindings(options),
        _ => unreachable!("operation 已由调用方核验"),
    }
}

fn parse_options(args: &[String]) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--write" {
            if options.write {
                return Err(CliError::new("--write 不能重复"));
            }
            options.write = true;
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

fn set_value(options: &mut Options, name: &str, value: &str) -> Result<(), CliError> {
    match name {
        "--arm-input" => set_path(&mut options.arm_input, name, value),
        "--fresh-target-verify" => set_path(&mut options.fresh_target_verify, name, value),
        "--side" => set_side(&mut options.side, name, value),
        "--id" => set_string(&mut options.id, name, value),
        "--work-dir" => set_path(&mut options.work_dir, name, value),
        "--reference-plan" => set_path(&mut options.reference_plan, name, value),
        "--backup-receipt" => set_path(&mut options.backup_receipt, name, value),
        "--comparison-sources" => set_path(&mut options.comparison_sources, name, value),
        "--fault-at" => set_string(&mut options.fault_at, name, value),
        "--target-plan" => set_path(&mut options.target_plan, name, value),
        "--record" => set_path(&mut options.record, name, value),
        "--output" => set_path(&mut options.output, name, value),
        _ => Err(CliError::new(format!("未知 recovery inputs 参数：{name}"))),
    }
}

fn build_reference(mut options: Options) -> Result<RecoveryInputsCommand, CliError> {
    let arm_input = input(&mut options.arm_input, "--arm-input")?;
    let fresh_target_verify = input(&mut options.fresh_target_verify, "--fresh-target-verify")?;
    let side = required(&mut options.side, "--side")?;
    let id = identifier(&mut options.id, "--id")?;
    let work_dir = path(
        required(&mut options.work_dir, "--work-dir")?,
        LocalTestPathKind::NewDirectory,
        "--work-dir",
    )?;
    let publication = publication(&mut options)?;
    ensure_empty(&options, "reference")?;
    Ok(RecoveryInputsCommand::Reference(ReferenceInputOptions {
        arm_input,
        fresh_target_verify,
        side,
        id,
        work_dir,
        publication,
    }))
}

fn build_product(mut options: Options) -> Result<RecoveryInputsCommand, CliError> {
    let reference_plan = input(&mut options.reference_plan, "--reference-plan")?;
    let backup_receipt = input(&mut options.backup_receipt, "--backup-receipt")?;
    let comparison_sources = input(&mut options.comparison_sources, "--comparison-sources")?;
    let arm_input = input(&mut options.arm_input, "--arm-input")?;
    let fresh_target_verify = input(&mut options.fresh_target_verify, "--fresh-target-verify")?;
    let side = required(&mut options.side, "--side")?;
    let id = identifier(&mut options.id, "--id")?;
    let fault_at = required(&mut options.fault_at, "--fault-at")?;
    validate_fault_at(&fault_at)?;
    let publication = publication(&mut options)?;
    ensure_empty(&options, "product")?;
    Ok(RecoveryInputsCommand::Product(ProductInputOptions {
        reference_plan,
        backup_receipt,
        comparison_sources,
        arm_input,
        fresh_target_verify,
        side,
        id,
        fault_at,
        publication,
    }))
}

fn build_bindings(mut options: Options) -> Result<RecoveryInputsCommand, CliError> {
    let reference_plan = input(&mut options.reference_plan, "--reference-plan")?;
    let target_plan = input(&mut options.target_plan, "--target-plan")?;
    let backup_receipt = input(&mut options.backup_receipt, "--backup-receipt")?;
    let record = input(&mut options.record, "--record")?;
    let publication = publication(&mut options)?;
    ensure_empty(&options, "bindings")?;
    Ok(RecoveryInputsCommand::Bindings(BindingsInputOptions {
        reference_plan,
        target_plan,
        backup_receipt,
        record,
        publication,
    }))
}

fn publication(options: &mut Options) -> Result<Option<RestoreInputPublication>, CliError> {
    match (options.output.take(), options.write) {
        (None, false) => Ok(None),
        (Some(output), true) => {
            options.write = false;
            Ok(Some(RestoreInputPublication {
                output: output_path(output)?,
            }))
        }
        _ => Err(CliError::new(
            "输入预览不落盘；发布必须同时指定 --output 与 --write",
        )),
    }
}

fn ensure_empty(options: &Options, operation: &str) -> Result<(), CliError> {
    let present = [
        ("--arm-input", options.arm_input.is_some()),
        (
            "--fresh-target-verify",
            options.fresh_target_verify.is_some(),
        ),
        ("--side", options.side.is_some()),
        ("--id", options.id.is_some()),
        ("--work-dir", options.work_dir.is_some()),
        ("--reference-plan", options.reference_plan.is_some()),
        ("--backup-receipt", options.backup_receipt.is_some()),
        ("--comparison-sources", options.comparison_sources.is_some()),
        ("--fault-at", options.fault_at.is_some()),
        ("--target-plan", options.target_plan.is_some()),
        ("--record", options.record.is_some()),
        ("--output", options.output.is_some()),
        ("--write", options.write),
    ];
    if let Some((name, _)) = present.into_iter().find(|(_, value)| *value) {
        Err(CliError::new(format!(
            "{name} 不属于 recovery inputs {operation}"
        )))
    } else {
        Ok(())
    }
}

fn input(target: &mut Option<PathBuf>, name: &str) -> Result<PathBuf, CliError> {
    path(
        required(target, name)?,
        LocalTestPathKind::ExistingFile,
        name,
    )
}

fn output_path(value: PathBuf) -> Result<PathBuf, CliError> {
    let value = path(value, LocalTestPathKind::OutputFile, "--output")?;
    if !value.parent().is_some_and(std::path::Path::is_dir) {
        return Err(CliError::new("--output 的父目录必须已经存在"));
    }
    Ok(value)
}

fn path(value: PathBuf, kind: LocalTestPathKind, label: &str) -> Result<PathBuf, CliError> {
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

fn identifier(target: &mut Option<String>, name: &str) -> Result<String, CliError> {
    let value = required(target, name)?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(CliError::new(format!(
            "{name} 必须以小写字母或数字开头，只包含小写字母、数字、下划线或连字符，最长 64 字节"
        )));
    }
    Ok(value)
}

fn validate_fault_at(value: &str) -> Result<(), CliError> {
    let Some(body) = value.strip_suffix('Z') else {
        return Err(CliError::new("--fault-at 必须使用 UTC Z 时间"));
    };
    let precision = body
        .split_once('.')
        .map(|(_, fraction)| fraction.len())
        .unwrap_or(0);
    if !matches!(precision, 0 | 3 | 6)
        || DateTime::parse_from_rfc3339(value).is_err()
        || body
            .split_once('.')
            .is_some_and(|(_, fraction)| !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(CliError::new(
            "--fault-at 必须是 UTC Z 格式，并使用秒、毫秒或微秒精度",
        ));
    }
    Ok(())
}

fn required<T>(target: &mut Option<T>, name: &str) -> Result<T, CliError> {
    target
        .take()
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {name}")))
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
    target: &mut Option<RestoreInputSide>,
    name: &str,
    value: &str,
) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(match value {
        "base" => RestoreInputSide::Base,
        "candidate" => RestoreInputSide::Candidate,
        _ => return Err(CliError::new("--side 只允许 base 或 candidate")),
    });
    Ok(())
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r', '\0'])
}

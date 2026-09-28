use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{
        RecoveryReferenceCommand, ReferencePlanAction, ReferencePlanOptions, ReferenceTargetInputs,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "scripts/restore_reference.py";
const PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_REFERENCE";
const INPUTS_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_INPUTS";
const FRESH_TARGET_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_FRESH_TARGET";
const SEED_SOURCE_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_SEED_SOURCE";

pub(crate) const USAGE: &str = "cargo xtask check recovery plan --plan <.local-tests 内参考计划> [--target-plan <现有目标计划> | --backup-receipt <文件> --comparison-sources <文件> --arm-input <文件> --fresh-target-verify <文件> --product-plan <文件> [--output <新文件> --write]]\n  cargo xtask check recovery check-dataset --plan <参考计划>\n  cargo xtask check recovery check-existing --plan <参考计划> [--side source|target]\n  cargo xtask check recovery dataset --plan <参考计划> --write\n  cargo xtask check recovery backup --plan <参考计划> --inventory <文件> --source-generation <文件> --source-export-result <文件> --write\n  cargo xtask check recovery restore --plan <参考计划> --backup-root <目录> --record <文件> --target-plan <文件> --runtime-registration <文件> --write\n  cargo xtask check recovery copy --plan <参考计划> --backup-root <目录> --copy-id <ID> --write\n  cargo xtask check recovery damage --plan <参考计划> --backup-root <目录> --artifact <相对路径> [--missing] --write";

pub(super) fn run(command: &RecoveryReferenceCommand, root: &Path) -> Result<()> {
    if matches!(command, RecoveryReferenceCommand::Help) {
        println!("{USAGE}");
        return Ok(());
    }
    let payload = protocol_at(command, root)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", SCRIPT],
        &[(PROTOCOL_KEY, payload.as_str())],
        &[
            PROTOCOL_KEY,
            INPUTS_PROTOCOL_KEY,
            FRESH_TARGET_PROTOCOL_KEY,
            SEED_SOURCE_PROTOCOL_KEY,
        ],
    )
}

pub(crate) fn protocol_at(command: &RecoveryReferenceCommand, root: &Path) -> Result<String> {
    let operation = command
        .operation()
        .ok_or("reference 帮助不启动私有阶段程序")?;
    let request = match command {
        RecoveryReferenceCommand::Help => unreachable!("帮助已在 operation 处拒绝"),
        RecoveryReferenceCommand::Plan(options) => plan_request(options, root)?,
        RecoveryReferenceCommand::CheckDataset { plan } => {
            simple_request(operation, plan, false, root)?
        }
        RecoveryReferenceCommand::CheckExisting { plan, side } => {
            let mut value = simple_request(operation, plan, false, root)?;
            value["side"] = json!(side.as_str());
            value
        }
        RecoveryReferenceCommand::Dataset { plan } => simple_request(operation, plan, true, root)?,
        RecoveryReferenceCommand::Backup {
            plan,
            inventory,
            source_generation,
            source_export_result,
        } => json!({
            "backend_dir": path_text(root, "后端目录")?,
            "format_version": 1,
            "inventory": input(inventory, root, "导出库存")?,
            "operation": operation,
            "plan": input(plan, root, "参考计划")?,
            "source_export_result": input(source_export_result, root, "来源导出结果")?,
            "source_generation": input(source_generation, root, "来源代次")?,
            "write": true,
        }),
        RecoveryReferenceCommand::Restore {
            plan,
            backup_root,
            record,
            target_plan,
            runtime_registration,
        } => json!({
            "backend_dir": path_text(root, "后端目录")?,
            "backup_root": directory(backup_root, root, "备份目录")?,
            "format_version": 1,
            "operation": operation,
            "plan": input(plan, root, "参考计划")?,
            "record": input(record, root, "恢复记录")?,
            "runtime_registration": input(runtime_registration, root, "运行时停止登记")?,
            "target_plan": input(target_plan, root, "目标计划")?,
            "write": true,
        }),
        RecoveryReferenceCommand::Copy {
            plan,
            backup_root,
            copy_id,
        } => {
            validate_identifier(copy_id, "副本 ID")?;
            json!({
                "backend_dir": path_text(root, "后端目录")?,
                "backup_root": directory(backup_root, root, "备份目录")?,
                "copy_id": copy_id,
                "format_version": 1,
                "operation": operation,
                "plan": input(plan, root, "参考计划")?,
                "write": true,
            })
        }
        RecoveryReferenceCommand::Damage {
            plan,
            backup_root,
            artifact,
            missing,
        } => {
            validate_artifact(artifact)?;
            json!({
                "artifact": artifact,
                "backend_dir": path_text(root, "后端目录")?,
                "backup_root": directory(backup_root, root, "备份目录")?,
                "format_version": 1,
                "missing": missing,
                "operation": operation,
                "plan": input(plan, root, "参考计划")?,
                "write": true,
            })
        }
    };
    if request["write"].as_bool() != Some(command.writes()) {
        return Err("reference 私有请求的写入授权与操作不一致".into());
    }
    serialize(json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-reference",
        "request": request,
    }))
}

fn plan_request(options: &ReferencePlanOptions, root: &Path) -> Result<Value> {
    let mut request = simple_request(
        "plan",
        &options.plan,
        matches!(options.action, ReferencePlanAction::Publish { .. }),
        root,
    )?;
    match &options.action {
        ReferencePlanAction::Summary => {}
        ReferencePlanAction::Verify { target_plan } => {
            request["target_plan"] = json!(input(target_plan, root, "目标计划")?);
        }
        ReferencePlanAction::Preview { inputs } => add_target_inputs(&mut request, inputs, root)?,
        ReferencePlanAction::Publish { inputs, output } => {
            add_target_inputs(&mut request, inputs, root)?;
            request["output"] = json!(output_file(output, root, "目标计划输出")?);
        }
    }
    Ok(request)
}

fn add_target_inputs(
    request: &mut Value,
    inputs: &ReferenceTargetInputs,
    root: &Path,
) -> Result<()> {
    request["backup_receipt"] = json!(input(&inputs.backup_receipt, root, "备份收据")?);
    request["comparison_sources"] = json!(input(&inputs.comparison_sources, root, "比较来源")?);
    request["arm_input"] = json!(input(&inputs.arm_input, root, "arm 输入")?);
    request["fresh_target_verify"] = json!(input(
        &inputs.fresh_target_verify,
        root,
        "fresh target 核验",
    )?);
    request["product_plan"] = json!(input(&inputs.product_plan, root, "产品恢复计划")?);
    Ok(())
}

fn simple_request(operation: &str, plan: &Path, write: bool, root: &Path) -> Result<Value> {
    Ok(json!({
        "backend_dir": path_text(root, "后端目录")?,
        "format_version": 1,
        "operation": operation,
        "plan": input(plan, root, "参考计划")?,
        "write": write,
    }))
}

fn input<'a>(value: &'a Path, root: &Path, label: &str) -> Result<&'a str> {
    local_path(value, root, LocalTestPathKind::ExistingFile, label)
}

fn directory<'a>(value: &'a Path, root: &Path, label: &str) -> Result<&'a str> {
    local_path(value, root, LocalTestPathKind::ExistingDirectory, label)
}

fn output_file<'a>(value: &'a Path, root: &Path, label: &str) -> Result<&'a str> {
    let text = local_path(value, root, LocalTestPathKind::OutputFile, label)?;
    if !value.parent().is_some_and(Path::is_dir) {
        return Err(format!("{label}的父目录必须已经存在").into());
    }
    Ok(text)
}

fn local_path<'a>(
    value: &'a Path,
    root: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<&'a str> {
    validate_local_test_path(value, root, kind).map_err(|error| format!("{label}无效：{error}"))?;
    path_text(value, label)
}

fn path_text<'a>(value: &'a Path, label: &str) -> Result<&'a str> {
    value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
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
        return Err(format!("{label}无效").into());
    }
    Ok(())
}

fn validate_artifact(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', ':', '\n', '\r', '\0'])
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err("损坏产物必须是备份根内的安全相对路径".into());
    }
    Ok(())
}

fn serialize(value: Value) -> Result<String> {
    let payload = serde_json::to_string(&value)?;
    if payload.len() > 65_536 || payload.contains(['\n', '\r', '\0']) {
        return Err("reference 私有协议为空、过长或包含换行符或 NUL".into());
    }
    Ok(payload)
}

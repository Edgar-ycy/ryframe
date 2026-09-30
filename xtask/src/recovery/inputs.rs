use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{
        BindingsInputOptions, ProductInputOptions, RecoveryInputsCommand, ReferenceInputOptions,
        RestoreInputPublication,
    },
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "tools/python/restore_input_plan.py";
const PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_INPUTS";
const REFERENCE_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_REFERENCE";
const FRESH_TARGET_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_FRESH_TARGET";
const SEED_SOURCE_PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_SEED_SOURCE";

pub(crate) const USAGE: &str = "cargo xtask check recovery inputs reference --arm-input <文件> --fresh-target-verify <文件> --side <base|candidate> --id <ID> --work-dir <新目录> [--output <新文件> --write]\n  cargo xtask check recovery inputs product --reference-plan <文件> --backup-receipt <文件> --comparison-sources <文件> --arm-input <文件> --fresh-target-verify <文件> --side <base|candidate> --id <ID> --fault-at <UTC Z 时间> [--output <新文件> --write]\n  cargo xtask check recovery inputs bindings --reference-plan <文件> --target-plan <文件> --backup-receipt <文件> --record <文件> [--output <新文件> --write]";

pub(super) fn run(command: &RecoveryInputsCommand, root: &Path) -> Result<()> {
    if matches!(command, RecoveryInputsCommand::Help) {
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
            REFERENCE_PROTOCOL_KEY,
            FRESH_TARGET_PROTOCOL_KEY,
            SEED_SOURCE_PROTOCOL_KEY,
        ],
    )
}

pub(crate) fn protocol_at(command: &RecoveryInputsCommand, root: &Path) -> Result<String> {
    let operation = command
        .operation()
        .ok_or("restore inputs 帮助不启动私有阶段程序")?;
    let mut request = match command {
        RecoveryInputsCommand::Help => unreachable!("帮助已在 operation 处拒绝"),
        RecoveryInputsCommand::Reference(options) => reference_request(options, root)?,
        RecoveryInputsCommand::Product(options) => product_request(options, root)?,
        RecoveryInputsCommand::Bindings(options) => bindings_request(options, root)?,
    };
    request["backend_dir"] = json!(path_text(root, "后端目录")?);
    request["format_version"] = json!(1);
    request["operation"] = json!(operation);
    request["write"] = json!(command.writes());
    serialize(json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-inputs",
        "request": request,
    }))
}

fn reference_request(options: &ReferenceInputOptions, root: &Path) -> Result<Value> {
    validate_identifier(&options.id)?;
    validate_local(
        &options.work_dir,
        root,
        LocalTestPathKind::NewDirectory,
        "参考计划工作目录",
    )?;
    let mut request = json!({
        "arm_input": input(&options.arm_input, root, "arm 输入")?,
        "fresh_target_verify": input(&options.fresh_target_verify, root, "fresh target 核验")?,
        "id": options.id,
        "side": options.side.as_str(),
        "work_dir": path_text(&options.work_dir, "参考计划工作目录")?,
    });
    add_publication(&mut request, options.publication.as_ref(), root)?;
    Ok(request)
}

fn product_request(options: &ProductInputOptions, root: &Path) -> Result<Value> {
    validate_identifier(&options.id)?;
    validate_fault_at(&options.fault_at)?;
    let mut request = json!({
        "arm_input": input(&options.arm_input, root, "arm 输入")?,
        "backup_receipt": input(&options.backup_receipt, root, "备份收据")?,
        "comparison_sources": input(&options.comparison_sources, root, "比较来源")?,
        "fault_at": options.fault_at,
        "fresh_target_verify": input(&options.fresh_target_verify, root, "fresh target 核验")?,
        "id": options.id,
        "reference_plan": input(&options.reference_plan, root, "参考计划")?,
        "side": options.side.as_str(),
    });
    add_publication(&mut request, options.publication.as_ref(), root)?;
    Ok(request)
}

fn bindings_request(options: &BindingsInputOptions, root: &Path) -> Result<Value> {
    let mut request = json!({
        "backup_receipt": input(&options.backup_receipt, root, "备份收据")?,
        "record": input(&options.record, root, "数据核验记录")?,
        "reference_plan": input(&options.reference_plan, root, "参考计划")?,
        "target_plan": input(&options.target_plan, root, "目标计划")?,
    });
    add_publication(&mut request, options.publication.as_ref(), root)?;
    Ok(request)
}

fn add_publication(
    request: &mut Value,
    publication: Option<&RestoreInputPublication>,
    root: &Path,
) -> Result<()> {
    if let Some(publication) = publication {
        validate_local(
            &publication.output,
            root,
            LocalTestPathKind::OutputFile,
            "输入发布文件",
        )?;
        if !publication.output.parent().is_some_and(Path::is_dir) {
            return Err("输入发布文件的父目录必须已经存在".into());
        }
        request["output"] = json!(path_text(&publication.output, "输入发布文件")?);
    }
    Ok(())
}

fn input<'a>(value: &'a Path, root: &Path, label: &str) -> Result<&'a str> {
    validate_local(value, root, LocalTestPathKind::ExistingFile, label)?;
    path_text(value, label)
}

fn validate_local(value: &Path, root: &Path, kind: LocalTestPathKind, label: &str) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(|error| format!("{label}无效：{error}").into())
}

fn path_text<'a>(value: &'a Path, label: &str) -> Result<&'a str> {
    value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}

fn validate_identifier(value: &str) -> Result<()> {
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
        return Err("恢复输入 ID 无效".into());
    }
    Ok(())
}

fn validate_fault_at(value: &str) -> Result<()> {
    let Some(body) = value.strip_suffix('Z') else {
        return Err("恢复故障时间必须使用 UTC Z 格式".into());
    };
    let precision = body
        .split_once('.')
        .map(|(_, fraction)| fraction.len())
        .unwrap_or(0);
    if !matches!(precision, 0 | 3 | 6)
        || chrono::DateTime::parse_from_rfc3339(value).is_err()
        || body
            .split_once('.')
            .is_some_and(|(_, fraction)| !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("恢复故障时间必须使用秒、毫秒或微秒精度".into());
    }
    Ok(())
}

fn serialize(value: Value) -> Result<String> {
    let payload = serde_json::to_string(&value)?;
    if payload.len() > 65_536 || payload.contains(['\n', '\r', '\0']) {
        return Err("restore inputs 私有协议为空、过长或包含换行符或 NUL".into());
    }
    Ok(payload)
}

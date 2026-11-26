use super::super::super::super::model::{
    CliError, CloneCommand, PostCopyOperation, PostCopyOptions, SeedRuntimeOperation,
    SeedRuntimeOptions,
};

use super::{optional_path, parse_options, require_write, required, run_dir};

pub(super) fn parse_post_copy(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(
        args,
        &[
            "--run-dir",
            "--operation",
            "--originals",
            "--request",
            "--producer-binding",
        ],
        false,
    )?;
    let operation = match required(&options, "--operation")? {
        "register" => PostCopyOperation::Register,
        "amend" => PostCopyOperation::Amend,
        "prepare" => PostCopyOperation::Prepare,
        "schedules" => PostCopyOperation::Schedules,
        "reconcile" => PostCopyOperation::Reconcile,
        "verify" => PostCopyOperation::Verify,
        "recover-session" => PostCopyOperation::RecoverSession,
        value => {
            return Err(CliError::new(format!(
                "post-copy --operation 无效：{value}"
            )));
        }
    };
    require_write("post-copy", &options, operation.effect().requires_write())?;
    let hold_originals = match options.values.get("--originals").map(String::as_str) {
        None => false,
        Some("held") if PostCopyOperation::supports_held_originals(operation.as_str()) => true,
        Some(_) => {
            return Err(CliError::new(
                "--originals held 仅适用于 Windows post-copy prepare、schedules 和 verify",
            ));
        }
    };
    let request = optional_path(&options, "--request")?;
    let producer_binding = optional_path(&options, "--producer-binding")?;
    if request.is_some()
        != matches!(
            operation,
            PostCopyOperation::Register | PostCopyOperation::Amend
        )
        || producer_binding.is_some() != matches!(operation, PostCopyOperation::RecoverSession)
    {
        return Err(CliError::new(
            "post-copy 的 request 或 producer binding 与操作不匹配",
        ));
    }
    Ok(CloneCommand::PostCopy(PostCopyOptions {
        run_dir: run_dir(&options)?,
        operation,
        hold_originals,
        request,
        producer_binding,
    }))
}

pub(super) fn parse_seed_runtime(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(
        args,
        &[
            "--run-dir",
            "--operation",
            "--originals",
            "--request",
            "--producer-binding",
        ],
        false,
    )?;
    let operation = seed_operation(required(&options, "--operation")?)?;
    require_write(
        "seed-runtime",
        &options,
        operation.effect().requires_write(),
    )?;
    let hold_originals = match options.values.get("--originals").map(String::as_str) {
        None => false,
        Some("held") if SeedRuntimeOperation::supports_held_originals(operation.as_str()) => true,
        Some(_) => {
            return Err(CliError::new(
                "seed-runtime --originals 只接受 held，且仅适用于 Windows 上的 quotas-plan、quotas-apply、departments-plan、departments-apply 和 departments-verify",
            ));
        }
    };
    let request = optional_path(&options, "--request")?;
    let producer_binding = optional_path(&options, "--producer-binding")?;
    let requires_request = matches!(
        operation,
        SeedRuntimeOperation::Register
            | SeedRuntimeOperation::ArmInput
            | SeedRuntimeOperation::IdentitiesReconcile
    );
    let permits_tool_build = matches!(
        operation,
        SeedRuntimeOperation::DepartmentsPlan | SeedRuntimeOperation::IdentitiesVerify
    );
    if !permits_tool_build && request.is_some() != requires_request
        || producer_binding.is_some() != matches!(operation, SeedRuntimeOperation::RecoverSession)
    {
        return Err(CliError::new(
            "seed-runtime 的 request 或 producer binding 与操作不匹配",
        ));
    }
    Ok(CloneCommand::SeedRuntime(SeedRuntimeOptions {
        run_dir: run_dir(&options)?,
        operation,
        hold_originals,
        request,
        producer_binding,
    }))
}

fn seed_operation(value: &str) -> Result<SeedRuntimeOperation, CliError> {
    Ok(match value {
        "register" => SeedRuntimeOperation::Register,
        "quotas-plan" => SeedRuntimeOperation::QuotasPlan,
        "quotas-apply" => SeedRuntimeOperation::QuotasApply,
        "quotas-reconcile" => SeedRuntimeOperation::QuotasReconcile,
        "departments-plan" => SeedRuntimeOperation::DepartmentsPlan,
        "departments-apply" => SeedRuntimeOperation::DepartmentsApply,
        "departments-reconcile" => SeedRuntimeOperation::DepartmentsReconcile,
        "departments-verify" => SeedRuntimeOperation::DepartmentsVerify,
        "identities-apply" => SeedRuntimeOperation::IdentitiesApply,
        "identities-reconcile" => SeedRuntimeOperation::IdentitiesReconcile,
        "identities-verify" => SeedRuntimeOperation::IdentitiesVerify,
        "prepare" => SeedRuntimeOperation::Prepare,
        "start" => SeedRuntimeOperation::Start,
        "close" => SeedRuntimeOperation::Close,
        "arm-input" => SeedRuntimeOperation::ArmInput,
        "stop" => SeedRuntimeOperation::Stop,
        "status" => SeedRuntimeOperation::Status,
        "recover" => SeedRuntimeOperation::Recover,
        "recover-session" => SeedRuntimeOperation::RecoverSession,
        _ => {
            return Err(CliError::new(format!(
                "seed-runtime --operation 无效：{value}"
            )));
        }
    })
}

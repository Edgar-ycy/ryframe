use std::{collections::BTreeMap, path::Path};

use crate::{local_test_path::LocalTestPathKind, workspace::root_dir};

use super::super::super::model::{
    CliError, LEGACY_B0_ADAPTER_CONTRACT, RUNTIME_USAGE, RuntimeBindOptions, RuntimeBuildOptions,
    RuntimeCommand, RuntimeControlInputs, RuntimeGenerationOptions, RuntimeOperation,
    RuntimeRecoverOptions, RuntimeRegistrationOptions, RuntimeSources, RuntimeStartOptions,
    RuntimeVerifyOptions,
};
use super::runtime_values::{
    commit_sha, coordinator_path, ensure_only, external_directory, parse_operation, parse_options,
    required, scoped_local_path, timeout, validate_write,
};

pub(super) fn parse(args: &[String]) -> Result<RuntimeCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new(RUNTIME_USAGE));
    };
    if matches!(operation.as_str(), "--help" | "-h") {
        return if values.is_empty() {
            Ok(RuntimeCommand::Help(None))
        } else {
            Err(CliError::new(RUNTIME_USAGE))
        };
    }
    let operation = parse_operation(operation)?;
    let parsed = parse_options(values)?;
    if parsed.help {
        return Ok(RuntimeCommand::Help(Some(operation)));
    }
    validate_write(operation, parsed.write)?;
    build_command(operation, &parsed.values)
}

fn build_command(
    operation: RuntimeOperation,
    values: &BTreeMap<&str, &str>,
) -> Result<RuntimeCommand, CliError> {
    match operation {
        RuntimeOperation::Build => build(values),
        RuntimeOperation::Register => register(values),
        RuntimeOperation::Start => start(values),
        RuntimeOperation::Status => status(values),
        RuntimeOperation::Stop => stop(values),
        RuntimeOperation::Recover => recover(values),
        RuntimeOperation::Bind => bind(values),
        RuntimeOperation::Verify => verify(values),
    }
}

fn build(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &[
            "--source-backend",
            "--expected-head",
            "--source-frontend",
            "--expected-frontend-head",
            "--adapter-contract",
            "--product-backend",
            "--output",
        ],
        "build",
    )?;
    let root = root_dir();
    let sources = sources(values)?;
    let expected_head = commit_sha(required(values, "--expected-head")?, "--expected-head")?;
    let expected_frontend_head = commit_sha(
        required(values, "--expected-frontend-head")?,
        "--expected-frontend-head",
    )?;
    let output = scoped_local_path(
        required(values, "--output")?,
        &root,
        &sources.source_backend,
        LocalTestPathKind::OutputFile,
        "构建收据",
    )?;
    Ok(RuntimeCommand::Build(RuntimeBuildOptions {
        sources,
        expected_head,
        expected_frontend_head,
        output,
    }))
}

fn register(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(values, &["--plan", "--target-plan", "--output"], "register")?;
    let root = root_dir();
    Ok(RuntimeCommand::Register(RuntimeRegistrationOptions {
        plan: coordinator_path(
            required(values, "--plan")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "参考计划",
        )?,
        target_plan: coordinator_path(
            required(values, "--target-plan")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "目标计划",
        )?,
        output: coordinator_path(
            required(values, "--output")?,
            &root,
            LocalTestPathKind::OutputFile,
            "运行登记",
        )?,
    }))
}

fn start(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &[
            "--runtime-registration",
            "--target-plan",
            "--source-backend",
            "--source-frontend",
            "--build-receipt",
            "--bindings",
            "--adapter-contract",
            "--product-backend",
            "--timeout",
        ],
        "start",
    )?;
    let root = root_dir();
    let control = control_inputs(values, &root)?;
    let sources = sources(values)?;
    let build_receipt = scoped_local_path(
        required(values, "--build-receipt")?,
        &root,
        &sources.source_backend,
        LocalTestPathKind::ExistingFile,
        "后端构建收据",
    )?;
    let bindings = coordinator_path(
        required(values, "--bindings")?,
        &root,
        LocalTestPathKind::ExistingFile,
        "数据绑定",
    )?;
    let timeout = timeout(values.get("--timeout").copied().unwrap_or("60"))?;
    Ok(RuntimeCommand::Start(RuntimeStartOptions {
        control,
        sources,
        build_receipt,
        bindings,
        timeout,
    }))
}

fn status(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &["--runtime-registration", "--target-plan"],
        "status",
    )?;
    control_inputs(values, &root_dir()).map(RuntimeCommand::Status)
}

fn stop(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &["--runtime-registration", "--target-plan", "--generation"],
        "stop",
    )?;
    generation_options(values).map(RuntimeCommand::Stop)
}

fn recover(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &[
            "--runtime-registration",
            "--target-plan",
            "--generation",
            "--owner",
        ],
        "recover",
    )?;
    let root = root_dir();
    let generation = generation_options_at(values, &root)?;
    let owner = coordinator_path(
        required(values, "--owner")?,
        &root,
        LocalTestPathKind::ExistingFile,
        "控制 owner",
    )?;
    Ok(RuntimeCommand::Recover(RuntimeRecoverOptions {
        generation,
        owner,
    }))
}

fn bind(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &[
            "--source-backend",
            "--source-frontend",
            "--adapter-contract",
            "--product-backend",
            "--build-receipt",
            "--launch-receipt",
            "--bindings",
            "--output",
        ],
        "bind",
    )?;
    let root = root_dir();
    let sources = sources(values)?;
    Ok(RuntimeCommand::Bind(RuntimeBindOptions {
        build_receipt: scoped_local_path(
            required(values, "--build-receipt")?,
            &root,
            &sources.source_backend,
            LocalTestPathKind::ExistingFile,
            "后端构建收据",
        )?,
        launch_receipt: coordinator_path(
            required(values, "--launch-receipt")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "启动收据",
        )?,
        bindings: coordinator_path(
            required(values, "--bindings")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "数据绑定",
        )?,
        output: coordinator_path(
            required(values, "--output")?,
            &root,
            LocalTestPathKind::OutputFile,
            "运行收据",
        )?,
        sources,
    }))
}

fn verify(values: &BTreeMap<&str, &str>) -> Result<RuntimeCommand, CliError> {
    ensure_only(
        values,
        &[
            "--source-backend",
            "--source-frontend",
            "--adapter-contract",
            "--product-backend",
            "--bindings",
            "--receipt",
        ],
        "verify",
    )?;
    let root = root_dir();
    Ok(RuntimeCommand::Verify(RuntimeVerifyOptions {
        sources: sources(values)?,
        bindings: coordinator_path(
            required(values, "--bindings")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "数据绑定",
        )?,
        receipt: coordinator_path(
            required(values, "--receipt")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "运行收据",
        )?,
    }))
}

fn sources(values: &BTreeMap<&str, &str>) -> Result<RuntimeSources, CliError> {
    let source_backend = external_directory(required(values, "--source-backend")?, "后端来源")?;
    let source_frontend = external_directory(required(values, "--source-frontend")?, "前端来源")?;
    let adapter_contract = values
        .get("--adapter-contract")
        .map(|value| (*value).to_owned());
    let product_backend = values
        .get("--product-backend")
        .map(|value| external_directory(value, "产品后端来源"))
        .transpose()?;
    match (&adapter_contract, &product_backend) {
        (None, None) => {}
        (Some(contract), Some(_)) if contract == LEGACY_B0_ADAPTER_CONTRACT => {}
        (Some(_), Some(_)) => return Err(CliError::new("未知的后端适配合同")),
        _ => {
            return Err(CliError::new(
                "--adapter-contract 与 --product-backend 必须成对提供",
            ));
        }
    }
    Ok(RuntimeSources {
        source_backend,
        source_frontend,
        adapter_contract,
        product_backend,
    })
}

fn control_inputs(
    values: &BTreeMap<&str, &str>,
    root: &Path,
) -> Result<RuntimeControlInputs, CliError> {
    Ok(RuntimeControlInputs {
        runtime_registration: coordinator_path(
            required(values, "--runtime-registration")?,
            root,
            LocalTestPathKind::ExistingFile,
            "运行登记",
        )?,
        target_plan: coordinator_path(
            required(values, "--target-plan")?,
            root,
            LocalTestPathKind::ExistingFile,
            "目标计划",
        )?,
    })
}

fn generation_options(values: &BTreeMap<&str, &str>) -> Result<RuntimeGenerationOptions, CliError> {
    generation_options_at(values, &root_dir())
}

fn generation_options_at(
    values: &BTreeMap<&str, &str>,
    root: &Path,
) -> Result<RuntimeGenerationOptions, CliError> {
    let generation = required(values, "--generation")?
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| CliError::new("--generation 必须是正整数"))?;
    Ok(RuntimeGenerationOptions {
        control: control_inputs(values, root)?,
        generation,
    })
}

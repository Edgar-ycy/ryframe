//! 普通runtime两角色构建；只发布本次独占副本，target仅作为Cargo输入产物。
use crate::{
    Result,
    cli::RuntimeBuildOptions,
    recovery::clone::native::{bridge, digest, json as document, source as inventory},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[path = "build/frontend.rs"]
mod frontend;
#[path = "build/readonly.rs"]
mod readonly;
#[path = "build/source.rs"]
pub(in crate::recovery) mod source;
pub(in crate::recovery) use readonly::request;

pub(super) trait Port {
    fn command(&mut self, root: &Path, program: &str, args: &[String]) -> Result<String>;
    fn context(&mut self, root: &Path) -> Result<Value>;
}
struct Native;
impl Port for Native {
    fn command(&mut self, root: &Path, program: &str, args: &[String]) -> Result<String> {
        let mut command = crate::process::child_command(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
            .args(args)
            .current_dir(root)
            .stderr(std::process::Stdio::inherit());
        for (name, _) in std::env::vars_os() {
            let upper = name.to_string_lossy().to_ascii_uppercase();
            if upper.starts_with("RYFRAME_")
                || upper.starts_with("VITE_")
                || upper.starts_with("SNOWFLAKE_")
                || upper.starts_with("APP_") && upper != "APP_ENV"
            {
                command.env_remove(name);
            }
        }
        if program == "cargo" {
            bridge::cargo_environment::configure_command(root, &mut command)?;
        }
        if program == "node" {
            command.stdout(std::process::Stdio::inherit());
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(crate::process::PreservedFailure::new(
                "正式恢复构建失败；保留产物和现场，不自动重放".into(),
                output.status.code(),
            )
            .into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }
    fn context(&mut self, root: &Path) -> Result<Value> {
        bridge::build_context::current(root)
    }
}

pub(super) fn run(
    coordinator: &Path,
    frontend: &Path,
    options: &RuntimeBuildOptions,
) -> Result<Value> {
    run_with(coordinator, frontend, options, &mut Native)
}

fn artifacts(output: &Path) -> Result<PathBuf> {
    let mut name = output
        .file_name()
        .ok_or("构建收据缺少文件名")?
        .to_os_string();
    name.push(".artifacts");
    Ok(output.with_file_name(name))
}

fn new_output(root: &Path, output: &Path) -> Result<PathBuf> {
    use crate::local_test_path::{
        LocalTestPathKind, validate_absolute_path, validate_local_test_path,
    };
    validate_local_test_path(output, root, LocalTestPathKind::OutputFile)?;
    validate_absolute_path(
        output.parent().ok_or("收据缺少父目录")?,
        LocalTestPathKind::ExistingDirectory,
    )?;
    let ignored = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--quiet", "--"])
        .arg(output)
        .status()?;
    if !ignored.success() {
        return Err("构建收据必须位于已忽略的本地目录".into());
    }
    let directory = artifacts(output)?;
    validate_local_test_path(&directory, root, LocalTestPathKind::NewDirectory)?;
    Ok(directory)
}

fn run_with(
    coordinator: &Path,
    frontend_tools: &Path,
    options: &RuntimeBuildOptions,
    port: &mut impl Port,
) -> Result<Value> {
    let root = source::repository(&options.sources.source_backend)?;
    let directory = new_output(&root, &options.output)?;
    let before = registered(coordinator, options)?;
    fs::create_dir(&directory)?;
    let front = if options.sources.adapter_contract.is_some() {
        frontend::run_b0(
            &options.sources.source_frontend,
            &options.expected_frontend_head,
        )?
    } else {
        frontend::run(
            frontend_tools,
            &options.sources.source_frontend,
            &options.expected_frontend_head,
            port,
        )?
    };
    if registered(coordinator, options)? != before {
        return Err("前端构建后后端来源变化，保留本次目录".into());
    }
    let receipt = backend(&root, &directory, port)?;
    if registered(coordinator, options)? != before {
        return Err("构建结束后产品或执行来源变化".into());
    }
    verify(&root, &receipt, &options.expected_head, port)?;
    front.verify()?;
    crate::local_test_path::validate_local_test_path(
        &directory,
        &root,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    crate::local_test_path::validate_local_test_path(
        &options.output,
        &root,
        crate::local_test_path::LocalTestPathKind::OutputFile,
    )?;
    // 副本目录已create-only拥有；收据也只创建，任何失败保留本次现场。
    document::write_new(&options.output, &receipt)?;
    if document::read(&options.output)? != receipt || registered(coordinator, options)? != before {
        return Err("构建收据发布后来源或收据变化；保留现场，不自动重放".into());
    }
    verify(&root, &receipt, &options.expected_head, port)?;
    front.verify()?;
    crate::local_test_path::validate_local_test_path(
        &options.output,
        &root,
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )?;
    crate::local_test_path::validate_local_test_path(
        &directory,
        &root,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    Ok(
        json!({"backend_receipt":options.output,"frontend_receipt":front.result["frontend_receipt"],
        "frontend_source":front.result["frontend_source"],"frontend_action":front.result["frontend_action"]}),
    )
}

fn registered(coordinator: &Path, options: &RuntimeBuildOptions) -> Result<Value> {
    source::registered(
        coordinator,
        &options.sources.source_backend,
        &options.expected_head,
        &json!(options.sources.adapter_contract),
        &json!(options.sources.product_backend),
    )
}

fn backend(root: &Path, directory: &Path, port: &mut impl Port) -> Result<Value> {
    let before = inventory::capture(root)?;
    if !before.source.snapshot.clean {
        return Err("恢复构建必须使用干净源码".into());
    }
    let context = port.context(root)?;
    let mut artifacts = serde_json::Map::new();
    crate::build_source::with_source_commit(root, &before.source.snapshot.head, || {
        for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
            let command = context["commands"][role]
                .as_array()
                .ok_or("构建上下文缺少角色命令")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "构建命令必须为字符串".into())
                })
                .collect::<Result<Vec<_>>>()?;
            let output = port.command(root, "cargo", &command[1..])?;
            let target = crate::recovery::full_stack::build::artifact(root, name, &output)?;
            let copy = crate::recovery::full_stack::build::freeze(&target, directory)?;
            let binding = digest::file(&copy)?;
            artifacts.insert(role.into(), json!({"executable":copy,"command":command,"bytes":binding.bytes,"sha256":binding.sha256}));
        }
        Ok(())
    })?;
    if inventory::capture(root)? != before || port.context(root)? != context {
        return Err("构建期间完整源码或工具链上下文变化，不能发布混合来源".into());
    }
    Ok(
        json!({"format_version":2,"kind":"restore-backend-build","sources":inventory::backend_domains(&before)?,"build":context,"artifacts":artifacts}),
    )
}

fn verify(root: &Path, receipt: &Value, expected: &str, port: &mut impl Port) -> Result<()> {
    let context = port.context(root)?;
    source::verify(root, receipt, expected, &context)?;
    if port.context(root)? != context {
        return Err("构建核验期间工具链或环境变化".into());
    }
    source::verify(root, receipt, expected, &context)
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime_build.rs"]
mod tests;

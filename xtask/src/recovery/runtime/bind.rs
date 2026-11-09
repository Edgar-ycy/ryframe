//! 在原恢复登记临界区内发布单侧 v3 运行收据。
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use super::{
    dataset_authority::{check, inputs::Inputs, path, reads::Window},
    generation, verify,
};
use crate::{
    Result,
    cli::{RuntimeBindOptions, RuntimeSources},
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{fixture_control::portable_path, inputs::publish::publish_verified},
};

#[path = "bind/control.rs"]
pub(super) mod control;
#[path = "bind/ownership.rs"]
pub(super) mod ownership;

use control::{Control, Operation};
use ownership::Owner;

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/bind.rs"]
mod tests;

pub(super) fn run(root: &Path, options: &RuntimeBindOptions) -> Result<Value> {
    let output = validate_output(root, &options.output)?;
    let control_root = ownership::control_root(root)?;
    let before = {
        let window = Window::direct()?;
        let mut inputs = Inputs::new(root);
        let result = Control::read(&mut inputs, &options.launch_receipt, Operation::Bind);
        window.finish(inputs.finish(result))?
    };
    validate_output_boundary(&output, &before.directory)?;
    Owner::with(&control_root, &before.operation, |owner| {
        let window = Window::direct()?;
        let mut inputs = Inputs::new(root);
        let result = bind_locked(root, options, &output, &before, owner, &mut inputs, &window);
        if result.is_err() {
            window.finish(inputs.finish(result))
        } else {
            result
        }
    })?;
    Ok(json!({"output":output}))
}

fn bind_locked(
    root: &Path,
    options: &RuntimeBindOptions,
    output: &Path,
    before: &Control,
    owner: &Owner,
    inputs: &mut Inputs,
    window: &Window,
) -> Result<()> {
    verify_control(owner, inputs, before, Operation::Bind)?;
    let paths = BoundPaths::read(options, &before.launch.value["request"])?;
    let runtime = runtime_receipt(&paths, before, inputs)?;
    let authority = &before.launch.value["request"]["authority"];
    verify::online(
        root,
        &paths.sources,
        &path(&runtime["paths"]["bindings"])?,
        &runtime,
        authority,
        inputs,
    )?;
    publish_verified(output, &runtime, || {
        verify_control(owner, inputs, before, Operation::Bind)?;
        window.finish(inputs.finish(Ok(true)))
    })
}

pub(super) fn verify_control(
    owner: &Owner,
    inputs: &mut Inputs,
    before: &Control,
    operation: Operation,
) -> Result<()> {
    owner.check()?;
    let observed = Control::read(inputs, &before.launch.path, operation)?;
    check(
        observed.snapshot == before.snapshot && observed.operation == before.operation,
        operation.drift(),
    )?;
    owner.check()
}

struct BoundPaths {
    sources: RuntimeSources,
    bindings: PathBuf,
    backend: PathBuf,
    frontend: PathBuf,
}

impl BoundPaths {
    fn read(options: &RuntimeBindOptions, request: &Value) -> Result<Self> {
        use LocalTestPathKind::{ExistingDirectory, ExistingFile};

        let selected = &options.sources;
        let roots = &request["roots"];
        let source_backend = registered_path(
            &selected.source_backend,
            &path(&roots["backend_execution"])?,
            ExistingDirectory,
        )?;
        let source_frontend = registered_path(
            &selected.source_frontend,
            &path(&roots["frontend"])?,
            ExistingDirectory,
        )?;
        let product = registered_path(
            selected
                .product_backend
                .as_deref()
                .unwrap_or(&selected.source_backend),
            &path(&roots["backend_product"])?,
            ExistingDirectory,
        )?;
        let documents = &request["paths"];
        let bindings = registered_path(
            &options.bindings,
            &path(&documents["bindings"])?,
            ExistingFile,
        )?;
        let backend = registered_path(
            &options.build_receipt,
            &path(&documents["backend_build"])?,
            ExistingFile,
        )?;
        let frontend = registered_path(
            &source_frontend.join("dist/.vite/restore-build.json"),
            &path(&documents["frontend_build"])?,
            ExistingFile,
        )?;
        Ok(Self {
            sources: RuntimeSources {
                source_backend,
                source_frontend,
                product_backend: selected.product_backend.as_ref().map(|_| product),
                adapter_contract: selected.adapter_contract.clone(),
            },
            bindings,
            backend,
            frontend,
        })
    }
}

pub(super) fn canonical_input(path: &Path, kind: LocalTestPathKind) -> Result<PathBuf> {
    validate_absolute_path(path, kind)?;
    let canonical = portable_path(&path.canonicalize()?);
    validate_absolute_path(path, kind)?;
    validate_absolute_path(&canonical, kind)?;
    Ok(canonical)
}

fn registered_path(selected: &Path, registered: &Path, kind: LocalTestPathKind) -> Result<PathBuf> {
    let selected = canonical_input(selected, kind)?;
    let registered = canonical_input(registered, kind)?;
    check(
        verify::same_path(&selected, &registered),
        "恢复运行 bind 显式输入与登记路径不是同一位置",
    )?;
    Ok(registered)
}

fn runtime_receipt(paths: &BoundPaths, control: &Control, inputs: &mut Inputs) -> Result<Value> {
    let bindings = inputs.read(&paths.bindings)?;
    let backend = inputs.read(&paths.backend)?;
    let frontend = inputs.read(&paths.frontend)?;
    let launch = &control.launch;
    let request = &launch.value["request"];
    let authority = &request["authority"];
    let mut processes = json!({});
    for role in generation::ROLES {
        let record = &launch.value["processes"][role]["process_receipt"];
        processes[role] = json!({"receipt_path":record["path"],"receipt_sha256":record["sha256"],"identity":record["identity"]});
    }
    let roots = &request["roots"];
    let documents = &request["paths"];
    Ok(json!({
        "format_version":3, "kind":"restore-runtime",
        "restore":{"id":authority["restore_id"],"backup_id":authority["backup_id"],
            "scope_id":authority["scope_id"],"plan_hash":authority["plan_hash"],
            "data_verified_at":authority["data_verified_at"]},
        "paths":{"backend_product_root":roots["backend_product"],"backend_execution_root":roots["backend_execution"],
            "frontend_root":roots["frontend"],"runtime_dir":launch.value["runtime_directory"],
            "bindings":documents["bindings"],"backend_build":documents["backend_build"],
            "frontend_build":documents["frontend_build"],"launch":launch.path},
        "digests":{"bindings":bindings.sha256(),"backend_build":backend.sha256(),
            "frontend_build":frontend.sha256(),"launch":launch.sha256()},
        "source":{"backup_source_sha":authority["backup_source_sha"],
            "backend_product_sha":authority["backend_product_sha"],"backend_execution_sha":authority["backend_execution_sha"],
            "backend_adapter_contract":authority["backend_adapter_contract"],"frontend_sha":authority["frontend_sha"]},
        "endpoints":{"api":authority["api_endpoint"],"worker":authority["worker_endpoint"],"frontend":authority["frontend_endpoint"]},
        "backend":backend.value,"frontend":frontend.value,"processes":processes,
    }))
}

pub(super) fn validate_output(root: &Path, output: &Path) -> Result<PathBuf> {
    validate_local_test_path(output, root, LocalTestPathKind::OutputFile)?;
    let parent = output.parent().ok_or("恢复运行收据缺少父目录")?;
    validate_absolute_path(parent, LocalTestPathKind::ExistingDirectory)?;
    let target = portable_path(&parent.canonicalize()?)
        .join(output.file_name().ok_or("恢复运行收据缺少文件名")?);
    let ignored = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--quiet", "--"])
        .arg(&target)
        .status()?;
    check(ignored.success(), "恢复运行收据必须使用 Git 已忽略的新文件")?;
    Ok(target)
}

fn validate_output_boundary(output: &Path, directory: &Path) -> Result<()> {
    let runtime = portable_path(&directory.canonicalize()?);
    check(
        !output
            .ancestors()
            .any(|ancestor| verify::same_path(ancestor, &runtime)),
        "恢复运行收据不能写入 lifecycle 管理的运行目录",
    )
}

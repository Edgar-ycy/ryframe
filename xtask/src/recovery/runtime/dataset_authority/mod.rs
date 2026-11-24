//! 首次浏览器会话前的静态运行权威模型；不执行产品或解释器。
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as strict},
        inputs::{
            document::{self, Document},
            reference::is_independent_b1,
        },
    },
};
use serde_json::Value;
use serde_json::json;
use std::path::{Component, PathBuf};

pub(crate) fn request(arguments: &Value) -> Result<Value> {
    authorized(arguments, |_, _, verified| Ok(verified.authority))
}

struct Verified {
    authority: Value,
    runtime: Value,
}

fn authorized<T>(
    arguments: &Value,
    use_verified: impl FnOnce(&std::path::Path, &mut inputs::Inputs, Verified) -> Result<T>,
) -> Result<T> {
    strict::exact(
        arguments,
        &["backend_dir", "runtime_receipt", "target_plan"],
        "恢复数据集权威参数",
    )?;
    let window = reads::Window::begin()?;
    let result = (|| {
        let backend = path(&arguments["backend_dir"])?;
        let actual = crate::recovery::fixture_control::git_root(&backend, "恢复数据集协调后端")?;
        check(
            actual == backend,
            "恢复数据集协调后端必须为实际 Git 工作树根",
        )?;
        let mut inputs = inputs::Inputs::new(&backend);
        let result = produce(&backend, &mut inputs, arguments)
            .and_then(|verified| use_verified(&backend, &mut inputs, verified));
        inputs.finish(result)
    })();
    window.finish(result)
}

fn produce(
    backend: &std::path::Path,
    inputs: &mut inputs::Inputs,
    arguments: &Value,
) -> Result<Verified> {
    let runtime = inputs.read(&path(&arguments["runtime_receipt"])?)?;
    schema::runtime(&runtime.value)?;
    let target = inputs.read(&path(&arguments["target_plan"])?)?;
    let documents = cross::read(inputs, &runtime.value)?;
    check(
        commands::same_path(
            backend,
            &path(&documents.launch.value["request"]["tools"]["coordinator_root"])?,
        )?,
        "恢复启动工具未绑定当前协调后端",
    )?;
    let reference = registration::reference(inputs, &documents.launch.value, &target)?;
    let (verified, plan, backup) = if is_independent_b1(&reference.value) {
        let projection = independent::read(
            backend,
            &reference,
            &target,
            &documents.launch.value["request"]["registration"]["registration"],
            &documents.bindings.value,
            inputs,
        )?;
        (projection.target, projection.reference, projection.backup)
    } else {
        let (chain, verified, _) =
            crate::recovery::inputs::target_plan::verify(backend, reference, &target.path)?;
        crate::recovery::source::evidence::comparison::verify(
            backend,
            &chain.comparison_document.value,
            true,
        )?;
        (verified, chain.reference.value, chain.backup)
    };
    let tools = commands::current_tools(backend, &std::env::current_exe()?, inputs)?;
    cross::verify(
        &runtime.value,
        &verified,
        &plan,
        &backup.manifest,
        &documents,
    )?;
    environment::verify(inputs, &documents.launch.value["request"])?;
    lifecycle::verify(inputs, &documents.launch, &plan, &tools)?;
    let dataset = lineage::extract(inputs, &backup.result)?;
    Ok(project(
        runtime,
        &target,
        &dataset,
        &plan["target"],
        &verified["product_execution"]["roots"]["execution_backend"],
    ))
}

fn project(
    runtime: Document,
    target: &Document,
    dataset: &Value,
    selected: &Value,
    execution_backend: &Value,
) -> Verified {
    Verified {
        authority: json!({
            "format_version": 1,
            "kind": "restore-dataset-authority",
            "runtime": document::binding(&runtime),
            "target_plan": document::binding(target),
            "source_generation": dataset["source_generation"],
            "dataset_lineage": dataset["dataset_lineage"],
            "target": {
                "scope_id": selected["scope_id"],
                "api_url": selected["api_url"],
                "frontend_url": selected["frontend_url"],
            },
            "execution_backend": execution_backend,
        }),
        runtime: runtime.value,
    }
}

pub(super) mod b0;
pub(crate) mod business;
pub(crate) mod commands;
pub(super) mod configuration_registration;
pub(super) mod cross;
pub(super) mod environment;
pub(super) mod independent;
pub(super) mod inputs;
mod lifecycle;
mod lineage;
mod live;
pub(in crate::recovery) mod monitoring;
pub(crate) mod reads;
pub(super) mod registration;
pub(super) mod request_shape;
mod role_configuration;
pub(super) mod schema;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/dataset_authority.rs"]
mod tests;
mod verified_target;
pub(crate) mod verify_existing;

pub(super) fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(strict::text(value, "恢复静态权威路径")?);
    check(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir)),
        "恢复静态权威路径必须为无父目录跳转的绝对路径",
    )?;
    check(
        !digest::linked_path(&path)?.contains(&true),
        "恢复静态权威路径不得经过链接或重解析点",
    )?;
    Ok(path)
}

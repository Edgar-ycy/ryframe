//! 从同一恢复登记、备份和构建生成正式启动请求及三角色命令。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::sources;
use crate::{
    Result,
    cli::RuntimeStartOptions,
    local_test_path::LocalTestPathKind::{ExistingDirectory, ExistingFile},
    recovery::{
        clone::native::digest,
        frontend_build,
        inputs::{
            chain::{self, Chain},
            document::{self, Document},
            target_plan,
        },
        runtime::{
            bind::canonical_input,
            dataset_authority::{
                check, commands, cross, environment, inputs::Inputs, path, registration,
            },
            status::Bound,
            verify::same_path,
        },
        source::evidence::comparison,
    },
};

#[derive(PartialEq, Eq)]
pub(super) struct Prepared {
    pub(super) bound: Bound,
    pub(super) request: Value,
    pub(super) commands: Value,
    pub(super) frontend_files: Value,
}

impl Prepared {
    pub(super) fn capture(
        backend: &Path,
        options: &RuntimeStartOptions,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let bound = Bound::read(backend, &options.control, inputs)?;
        let (chain, target) = verified_target(backend, &bound, inputs)?;
        let bindings = inputs.read(&canonical_input(&options.bindings, ExistingFile)?)?;
        let execution = &target["product_execution"];
        let backend_build = registered_build(
            inputs,
            &options.build_receipt,
            &execution["builds"]["backend"],
        )?;
        let frontend_root = canonical_input(&options.sources.source_frontend, ExistingDirectory)?;
        let frontend_path = frontend_root.join("dist/.vite/restore-build.json");
        let frontend_build =
            registered_build(inputs, &frontend_path, &execution["builds"]["frontend"])?;
        let authority = cross::start_authority(
            &bindings.value,
            &target,
            &chain.reference.value,
            &chain.backup.manifest,
        )?;
        bound.check_stop_authority(&authority)?;
        let build_environment = &chain::source_arm(&chain)?["backend_environment"];
        let environment_binding = document::binding_shape(build_environment, "已登记后端构建环境")?;
        inputs.protect(&environment_binding.path)?;
        let roots = sources::read(
            backend,
            &options.sources,
            execution,
            &backend_build,
            &frontend_build,
            &authority,
            build_environment,
        )?;
        let tools = commands::current_tools(backend, &std::env::current_exe()?, inputs)?;
        let mut request = json!({
            "format_version":2,"kind":"restore-runtime-launch-request",
            "runtime_directory":bound.runtime,"authority":authority,"registration":bound.binding,
            "roots":roots,"tools":tools,"artifacts":backend_build.value["artifacts"],
            "paths":{"bindings":bindings.path,"backend_build":backend_build.path,"frontend_build":frontend_build.path},
            "digests":{"bindings":bindings.sha256(),"backend_build":backend_build.sha256(),"frontend_build":frontend_build.sha256()},
        });
        request["environment"] = environment::capture(inputs, &request)?;
        let commands = commands::expected(&request, &request["tools"])?;
        let frontend_files = frontend_build::artifact_snapshot(&frontend_root)?;
        pin_execution(&request, inputs)?;
        Ok(Self {
            bound,
            request,
            commands,
            frontend_files,
        })
    }
}

fn registered_build(inputs: &mut Inputs, selected: &Path, descriptor: &Value) -> Result<Document> {
    let original = document::binding_shape(descriptor, "登记启动构建收据")?;
    check(
        same_path(
            &canonical_input(selected, ExistingFile)?,
            &canonical_input(&original.path, ExistingFile)?,
        ),
        "恢复启动必须使用原登记位置的构建收据",
    )?;
    let registered = inputs.read(&original.path)?;
    check(
        document::binding(&registered) == *descriptor,
        "恢复启动构建收据与原登记摘要不同",
    )?;
    Ok(registered)
}

fn verified_target(backend: &Path, bound: &Bound, inputs: &mut Inputs) -> Result<(Chain, Value)> {
    let registered = inputs.bound(&bound.binding["registration"])?;
    let target = inputs.bound(&bound.binding["target_plan"])?;
    let reference = registration::read_binding(inputs, &bound.binding, &target, &registered)?;
    let (chain, verified, _) = target_plan::verify(backend, reference, &target.path)?;
    check(verified == target.value, "启动目标计划与原登记不同")?;
    comparison::verify(backend, &chain.comparison_document.value, true)?;
    Ok((chain, verified))
}

fn pin_execution(request: &Value, inputs: &mut Inputs) -> Result<()> {
    for role in ["api", "worker"] {
        let artifact = &request["artifacts"][role];
        pin(inputs, path(&artifact["executable"])?, artifact)?;
        let configuration = &request["environment"]["configurations"][role];
        let directory = path(&configuration["directory"])?;
        for (name, binding) in crate::recovery::clone::native::json::object(
            &configuration["files"],
            "恢复启动配置文件",
        )? {
            pin(inputs, directory.join(name), binding)?;
        }
    }
    Ok(())
}

fn pin(inputs: &mut Inputs, path: PathBuf, expected: &Value) -> Result<()> {
    inputs.protect(&path)?;
    let observed = digest::file(&path)?;
    check(
        expected["bytes"] == observed.bytes && expected["sha256"] == observed.sha256,
        "恢复启动产物或配置与原摘要不同",
    )
}

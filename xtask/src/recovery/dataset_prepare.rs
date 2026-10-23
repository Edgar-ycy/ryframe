//! 恢复参考数据集原生入口；请求与凭据只消费受管文件或有界标准输入。
use super::clone::native::json as documents;
use crate::{
    Result,
    cli::{DatasetPrepareCommand, DatasetPrepareMode},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{io::Read, path::Path};
#[path = "dataset_prepare/batch.rs"]
mod batch;
#[path = "dataset_prepare/configuration.rs"]
mod configuration;
#[path = "dataset_prepare/existing.rs"]
mod existing;
#[path = "dataset_prepare/existing_native.rs"]
mod existing_native;
#[path = "dataset_prepare/model.rs"]
mod model;
#[path = "dataset_prepare/prepare.rs"]
mod prepare;
#[path = "dataset_prepare/session.rs"]
mod session;
#[cfg(test)]
#[path = "dataset_prepare/tests.rs"]
mod tests;
const KIND: &str = "ryframe-xtask-recovery-dataset-prepare";
pub(crate) const MARKER: &str = "__recovery-dataset";

pub(super) fn run(command: &DatasetPrepareCommand, root: &Path) -> Result<()> {
    let DatasetPrepareCommand::Run(options) = command else {
        println!(
            "cargo xtask check recovery dataset-prepare --plan <绝对文件> <--preflight <绝对文件>|--verify-existing <绝对文件> [--side source|target]> --write"
        );
        return Ok(());
    };
    let request = private_invocation_at(options, root)?;
    execute(&request, root)
}

pub(crate) fn private_invocation_at(
    options: &crate::cli::DatasetPrepareOptions,
    root: &Path,
) -> Result<Value> {
    validate_local_test_path(&options.plan, root, LocalTestPathKind::ExistingFile)?;
    let (preflight, existing, side) = match &options.mode {
        DatasetPrepareMode::Prepare { preflight } => (Some(preflight), None, "source"),
        DatasetPrepareMode::VerifyExisting { dataset, side } => {
            (None, Some(dataset), side.as_str())
        }
    };
    for path in [preflight, existing].into_iter().flatten() {
        validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
    }
    Ok(
        json!({"backend_dir":root,"format_version":1,"kind":KIND,"plan":options.plan,
        "preflight":preflight,"side":side,"verify_existing":existing,"write":true}),
    )
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| {
        arg == existing_native::CLONE_MARKER || arg == existing_native::SOURCE_MARKER
    }) {
        return Some(existing_native::run(&args));
    }
    if args.first().is_none_or(|arg| arg != MARKER) {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err("数据集私有入口不接受命令行取值".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin().take(32769).read_to_end(&mut bytes)?;
        if bytes.len() > 32768 {
            return Err("数据集私有请求超过32KiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| "数据集私有请求必须为UTF-8")?;
        let request = documents::parse_fragment(text, "数据集私有请求")?;
        let root = std::env::current_dir()?;
        execute(&request, &root)
    })();
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "原生恢复数据集失败：{error}；保留预检、阶段与创建收据；写入结果可能未知，禁止自动重放。"
            );
            1
        }
    })
}

fn execute(request: &Value, root: &Path) -> Result<()> {
    let inputs = model::Inputs::read(request, root)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = if inputs.preflight.is_some() {
        runtime.block_on(prepare::run(&inputs))?
    } else {
        existing::ownership(&inputs)?;
        runtime.block_on(existing::run(&inputs))?
    };
    inputs.unchanged()?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

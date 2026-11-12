//! 最终证明与监控共享的原运行代次锁内核验。
use std::path::Path;

use serde_json::Value;

use super::{
    bind::{
        control::{Control, Operation},
        ownership::{Owner, control_root},
        verify_control,
    },
    dataset_authority::{
        check, commands,
        inputs::Inputs,
        path,
        reads::{self, Window},
        schema,
    },
    verify,
};
use crate::{
    Result,
    cli::RuntimeVerifyOptions,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::inputs::document::Document,
};

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/live_verify.rs"]
mod tests;

/// 调用方持有同线程读取窗口，控制快照、输入窗口和 owner 均在释放锁前完成后验。
pub(in crate::recovery) fn verify_live_generation(
    root: &Path,
    options: &RuntimeVerifyOptions,
    authority: &Value,
    receipt_sha256: &str,
    window: &Window,
) -> Result<Value> {
    verify_live_generation_with(root, options, receipt_sha256, window, |receipt, inputs| {
        verify::online(
            root,
            &options.sources,
            &options.bindings,
            &receipt.value,
            authority,
            inputs,
        )
        .map(|observed| observed.reply(receipt))
    })
}

pub(super) fn verify_live_generation_with(
    root: &Path,
    options: &RuntimeVerifyOptions,
    receipt_sha256: &str,
    window: &Window,
    verify_online: impl FnOnce(&Document, &mut Inputs) -> Result<Value>,
) -> Result<Value> {
    super::validate_sources(&options.sources)?;
    validate_local_test_path(&options.bindings, root, LocalTestPathKind::ExistingFile)?;
    // 浏览器会把同一份原始收据复制到独立 runner，业务证明已核对其固定证据路径。
    validate_absolute_path(&options.receipt, LocalTestPathKind::ExistingFile)?;
    let control = control_root(root)?;
    reads::directory(&control)?;
    let mut inputs = Inputs::new(root);
    let receipt = inputs.read(&options.receipt)?;
    check(
        receipt.sha256() == receipt_sha256,
        "恢复运行收据的实际 SHA-256 与给定摘要不同",
    )?;
    schema::runtime(&receipt.value)?;
    let runtime = &receipt.value;
    let launch = path(&runtime["paths"]["launch"])?;
    let before = Control::read(&mut inputs, &launch, Operation::Verify)?;
    check(
        commands::same_path(
            &path(&runtime["paths"]["runtime_dir"])?,
            &path(&before.launch.value["runtime_directory"])?,
        )? && runtime["digests"]["launch"] == before.launch.sha256(),
        "恢复运行收据与原启动目录或原启动收据字节不同",
    )?;
    Owner::with(&control, &before.operation, |owner| {
        let result = (|| {
            verify_control(owner, &mut inputs, &before, Operation::Verify)?;
            let observed = verify_online(&receipt, &mut inputs);
            let after = verify_control(owner, &mut inputs, &before, Operation::Verify);
            match (observed, after) {
                (Ok(observed), Ok(())) => Ok(observed),
                (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
                (Err(error), Err(after)) => {
                    Err(format!("{error}；恢复运行控制后验同时失败：{after}").into())
                }
            }
        })();
        window.finish_runtime_owner(owner, inputs.finish(result))
    })
}

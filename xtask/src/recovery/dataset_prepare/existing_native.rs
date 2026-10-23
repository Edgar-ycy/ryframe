//! 已有数据的原生持久生产者；参数仅包含受管路径和运行身份。
use std::ffi::OsString;

#[path = "existing_native/clone.rs"]
mod clone;
#[path = "existing_native/common.rs"]
mod common;
#[path = "existing_native/source.rs"]
pub(in crate::recovery) mod source;

pub(crate) const CLONE_MARKER: &str = "__recovery-existing-clone";
pub(crate) const SOURCE_MARKER: &str = "__recovery-existing-source";

pub(crate) fn run(args: &[OsString]) -> i32 {
    let marker = args.first().and_then(|value| value.to_str());
    let result = match marker {
        Some(CLONE_MARKER) => clone::run(&args[1..]),
        Some(SOURCE_MARKER) => source::run(&args[1..]),
        _ => Err("已有数据私有入口无效".into()),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "原生已有数据核验失败：{error}；保留进程、输入和失败证据；远端会话结果可能未知，禁止自动重放。"
            );
            1
        }
    }
}

#[cfg(test)]
#[path = "existing_native/tests.rs"]
mod tests;

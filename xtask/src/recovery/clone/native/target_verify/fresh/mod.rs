//! fresh-target 的完整只读状态观察，不获取写锁或修复阶段文件。
mod status;
mod storage;
pub(crate) use status::observe;

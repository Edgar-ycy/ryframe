//! 备份文件与对象存储的只读校验实现。

mod artifacts;
mod objects;
pub use artifacts::artifact_verifier;
pub use objects::object_verifier;

//! 追加迁移命令入口与安全写入接口。

#[path = "migration/model.rs"]
mod model;
#[path = "migration/plan.rs"]
mod plan;
#[path = "migration/transaction.rs"]
mod transaction;

#[allow(unused_imports)]
pub(crate) use plan::run;

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use model::{FileOperations, PlannedWrite};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use plan::create_migration;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use transaction::commit_writes_with;

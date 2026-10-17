//! 追加迁移命令入口与安全写入接口。

#[path = "migration/history.rs"]
mod history;
#[path = "migration/model.rs"]
mod model;
#[path = "migration/plan.rs"]
mod plan;
#[path = "migration/transaction.rs"]
mod transaction;

#[allow(unused_imports)]
pub(crate) use plan::run;

#[allow(unused_imports)]
pub(crate) use history::{
    LockedFile, check_history, freeze_history, load_lock, lock_identity, parse_lock,
    refresh_history_baseline, run_check, run_freeze, run_reconcile, run_refresh_baseline,
    safe_locked_path, sha256, verify_append_only, verify_generated_registry_wiring, verify_lock,
    write_lock_atomically,
};
#[allow(unused_imports)]
pub(crate) use model::{FileOperations, PlannedWrite};
#[allow(unused_imports)]
pub(crate) use plan::{create_migration, migration_run_args};
#[allow(unused_imports)]
pub(crate) use transaction::commit_writes_with;

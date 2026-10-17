#[path = "history/append.rs"]
mod append;
#[path = "history/command.rs"]
mod command;
#[path = "history/discover.rs"]
mod discover;
#[path = "history/git.rs"]
mod git;
#[path = "history/lock.rs"]
mod lock;
#[path = "history/model.rs"]
mod model;
#[path = "history/trusted.rs"]
mod trusted;

#[allow(unused_imports)]
pub(crate) use append::verify_append_only;
#[allow(unused_imports)]
pub(crate) use command::{
    check as check_history, freeze as freeze_history, refresh_baseline as refresh_history_baseline,
    run_check, run_freeze, run_reconcile, run_refresh_baseline,
};
#[allow(unused_imports)]
pub(crate) use discover::verify_generated_registry_wiring;
#[allow(unused_imports)]
pub(crate) use lock::{load_lock, parse_lock, verify_lock, write_lock_atomically};
#[allow(unused_imports)]
pub(crate) use model::{LockedFile, lock_identity, safe_locked_path, sha256};

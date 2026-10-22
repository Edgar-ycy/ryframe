//! 登录预算纯模型与离线文件互操作；不执行 HTTP 或真实登录。

#[path = "login_budget/files.rs"]
mod files;

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use tempfile::TempDir;

use crate::login_budget::{Binding, BudgetError, LedgerFile, Reservation};

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/login-budget-files");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let path = directory.path().join("budget.json");
        Self {
            _directory: directory,
            path,
        }
    }

    fn ledger(&self) -> LedgerFile {
        LedgerFile::new(
            self.path.clone(),
            Binding::new("login-test", 5, 60_000).unwrap(),
        )
        .unwrap()
    }

    fn read(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.path).unwrap()).unwrap()
    }

    fn reserve(&self, ledger: &LedgerFile, now: u64) -> Vec<Reservation> {
        ledger
            .try_reserve("system", "admin", "198.18.1.2", now)
            .unwrap()
            .reservation
            .unwrap()
    }

    fn attempt(&self, ledger: &LedgerFile, now: u64) {
        ledger.complete(&self.reserve(ledger, now), now).unwrap();
    }
}

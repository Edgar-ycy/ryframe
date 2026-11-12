use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{digest, source},
        fixture_control::portable_path,
        frontend_build::{recovery_request, tests as fixture},
    },
};

pub(super) struct Frontend {
    directory: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) head: String,
    pub(super) receipt: PathBuf,
    pub(super) receipt_sha256: String,
    pub(super) files: Value,
}

impl Frontend {
    pub(super) fn new() -> Result<Self> {
        let directory = fixture::fixture();
        let root = portable_path(&directory.path().canonicalize()?);
        let inventory = source::inventory_value(&source::capture(&root)?);
        let head = inventory["source"]["snapshot"]["head"]
            .as_str()
            .ok_or("前端测试夹具缺少 Git 提交")?
            .to_owned();
        fixture::write_receipt(&root, &head, &inventory);
        let receipt = portable_path(&root.join("dist/.vite/restore-build.json").canonicalize()?);
        let receipt_sha256 = digest::file(&receipt)?.sha256;
        let files = recovery_request(&json!({
            "action": "artifacts",
            "root": root,
            "value": null,
            "expected_head": null,
            "inventory": null,
        }))?;

        Ok(Self {
            directory,
            root,
            head,
            receipt,
            receipt_sha256,
            files,
        })
    }

    pub(super) fn keep(self) -> PathBuf {
        self.directory.keep()
    }
}

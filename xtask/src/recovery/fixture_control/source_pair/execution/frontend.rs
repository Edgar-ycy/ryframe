use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, strip_local_test_prefix, validate_absolute_path},
    recovery::fixture_control::portable_path,
};

use super::{git, input, root_receipt, source, window};

pub(in crate::recovery) struct FrontendPublication {
    original: PathBuf,
    destination: PathBuf,
    inventory: source::Inventory,
    receipt: Value,
}

/// 将冻结的前端生成差异发布为原 HEAD 的单父后继。
pub(in crate::recovery) fn publish_frontend(
    root: &Path,
    destination: &Path,
    expected: &Value,
) -> Result<FrontendPublication> {
    validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
    let original = portable_path(&fs::canonicalize(root)?);
    let top = git::text(&original, &["rev-parse", "--show-toplevel"])?;
    if portable_path(&fs::canonicalize(top)?) != original {
        return Err("派生发布必须绑定前端实际 Git 根目录".into());
    }
    let destination = destination_root(&original, destination)?;
    let side = input::capture(&original, expected)?;
    if side.patch.is_empty() && side.inventory.source.snapshot.files.is_empty() {
        return Err("前端派生发布必须包含已登记的生成差异".into());
    }
    git::publish(&side, &destination)?;
    let receipt = published_receipt(&destination, &side.inventory, true)?;
    let publication = FrontendPublication {
        original,
        destination,
        inventory: side.inventory,
        receipt,
    };
    publication.check_original()?;
    Ok(publication)
}

impl FrontendPublication {
    pub(in crate::recovery) fn head(&self) -> &str {
        self.receipt["head"]
            .as_str()
            .expect("前端发布收据必须包含已核验的 HEAD")
    }

    pub(in crate::recovery) fn verify(&self) -> Result<Value> {
        self.check_original()?;
        let result =
            published_receipt(&self.destination, &self.inventory, false).and_then(|receipt| {
                window::check(receipt == self.receipt, "已发布前端源码收据发生变化")?;
                Ok(receipt)
            });
        let original = self.check_original();
        match result {
            Ok(receipt) => original.map(|()| receipt),
            Err(error) => Err(window::failure(error, original)),
        }
    }

    fn check_original(&self) -> Result<()> {
        validate_absolute_path(&self.original, LocalTestPathKind::ExistingDirectory)?;
        window::check(
            source::capture(&self.original)? == self.inventory,
            "原始前端工作树或完整生成源码发生变化",
        )
    }
}

fn destination_root(original: &Path, destination: &Path) -> Result<PathBuf> {
    validate_absolute_path(destination, LocalTestPathKind::NewDirectory)?;
    let parent = destination.parent().ok_or("派生前端缺少输出父目录")?;
    let name = destination.file_name().ok_or("派生前端缺少输出目录名")?;
    let parent = portable_path(&fs::canonicalize(parent)?);
    let destination = parent.join(name);
    if strip_local_test_prefix(&destination, original).is_some()
        || strip_local_test_prefix(original, &destination).is_some()
    {
        return Err("派生前端与原始前端根目录不能重叠".into());
    }
    if parent.join("fixture.json").try_exists()? {
        return Err("派生前端的输出父目录不能含 fixture.json".into());
    }
    Ok(destination)
}

fn published_receipt(root: &Path, original: &source::Inventory, initial: bool) -> Result<Value> {
    let mut receipt = root_receipt(root, &original.files, "frontend", initial)?;
    let head = receipt["head"].as_str().ok_or("派生前端收据缺少 HEAD")?;
    let parent = original.source.snapshot.head.as_str();
    let ancestry = git::text(root, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
    let mut commits = ancestry.split_ascii_whitespace();
    if commits.next() != Some(head) || commits.next() != Some(parent) || commits.next().is_some() {
        return Err("派生前端必须是冻结原始 HEAD 的单父后继".into());
    }
    receipt["parent"] = json!(parent);
    Ok(receipt)
}

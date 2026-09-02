use std::path::Path;

use crate::{Result, source_edit::SourceEdit};

pub(crate) fn with_source_edit<T>(
    path: &Path,
    label: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let (mut edit, _) = SourceEdit::apply(path, label, "//")?;
    let outcome = operation();
    let restore = edit.restore();
    match (outcome, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(operation_error), Err(restore_error)) => Err(format!(
            "增量测量命令失败：{operation_error}；源码还原也失败：{restore_error}"
        )
        .into()),
    }
}

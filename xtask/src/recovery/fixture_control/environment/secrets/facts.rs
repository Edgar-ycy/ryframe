use super::super::super::{file_binding, path_text_canonical, read_json, validate_path};
use super::super::protocol::GuardError;
use crate::recovery::clone::native::source;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Fixture {
    path: PathBuf,
    pub(super) execution: PathBuf,
    pub(super) binding: Value,
    generated: Value,
}

pub(super) fn snapshot(root: &Path) -> Result<Value> {
    Ok(source::protocol::request(&json!({"action":"snapshot","root":root}))?["snapshot"].clone())
}

impl Fixture {
    pub(super) fn open(root: &Path, path: &Path) -> Result<Self> {
        validate_path(path, root, LocalTestPathKind::ExistingFile, "业务夹具收据")?;
        let binding = file_binding(path, "业务夹具收据")?;
        let value = read_json(path, "业务夹具收据", 16 * 1024 * 1024)?;
        super::super::guards::fixture(&value)?;
        let execution = PathBuf::from(
            value["paths"]["backend"]
                .as_str()
                .ok_or("夹具后端路径缺失")?,
        );
        validate_absolute_path(&execution, LocalTestPathKind::ExistingDirectory)?;
        if execution != root {
            validate_path(
                &execution,
                root,
                LocalTestPathKind::ExistingDirectory,
                "夹具执行目录",
            )?;
        }
        path_text_canonical(&execution, "夹具后端")?;
        let generated = value["generated"]["backend"].clone();
        let result = Self {
            path: path.to_path_buf(),
            execution,
            binding,
            generated,
        };
        result.check()?;
        Ok(result)
    }

    pub(super) fn check(&self) -> Result<()> {
        if file_binding(&self.path, "业务夹具收据")? != self.binding {
            return Err(Box::new(GuardError("业务夹具收据绑定发生变化")));
        }
        if snapshot(&self.execution)? != self.generated {
            return Err(Box::new(GuardError("业务夹具生成后源码快照发生变化")));
        }
        Ok(())
    }
}

pub(super) fn output(root: &Path, execution: &Path, output: &Path) -> Result<()> {
    validate_path(output, root, LocalTestPathKind::NewDirectory, "秘密输出")?;
    let boundary = execution.join(".local-tests/reference-fixture");
    if output == boundary || !output.starts_with(&boundary) {
        return Err(Box::new(GuardError(
            "秘密输出必须属于执行工作树 reference-fixture 子目录",
        )));
    }
    let parent = output.parent().ok_or("秘密输出缺少父目录")?;
    validate_path(
        parent,
        root,
        LocalTestPathKind::ExistingDirectory,
        "秘密输出父目录",
    )?;
    if fs::symlink_metadata(output).is_ok() {
        return Err(Box::new(GuardError("秘密输出已存在，禁止覆盖")));
    }
    Ok(())
}

//! 环境输出同时属于协调仓与冻结执行树，校验阶段不创建任何文件。
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) struct OutputError(String);
impl std::fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for OutputError {}

pub(super) fn validate(root: &Path, fixture: &Value, output: &Path) -> Result<PathBuf> {
    super::guards::fixture(fixture)?;
    let execution = Path::new(
        fixture["paths"]["backend"]
            .as_str()
            .ok_or("夹具执行目录缺失")?,
    );
    validate_local_test_path(execution, root, LocalTestPathKind::ExistingDirectory)?;
    let boundary = execution.join(".local-tests").join("reference-fixture");
    let checked = (|| -> Result<()> {
        validate_local_test_path(output, root, LocalTestPathKind::NewDirectory)?;
        validate_local_test_path(output, execution, LocalTestPathKind::NewDirectory)?;
        if output == boundary || !output.starts_with(&boundary) {
            return Err("环境输出不属于本代执行树的参考夹具目录".into());
        }
        Ok(())
    })();
    checked.map_err(|_| OutputError(format!("环境输出必须是协调仓内执行工作树 .local-tests/reference-fixture 下父目录已存在、无链接且不可覆盖的新目录；正确绝对路径示例：{}", boundary.join("environment-seed-r1").display())))?;
    Ok(output.to_owned())
}

pub(super) fn request(args: &Value) -> Result<Value> {
    crate::recovery::clone::native::json::exact(
        args,
        &["backend", "plan", "output"],
        "环境准备输出核验",
    )?;
    let root = Path::new(args["backend"].as_str().ok_or("协调后端目录缺失")?);
    let loaded = super::plan::inputs(&json!({"backend":args["backend"],"plan":args["plan"]}))?;
    let output = Path::new(args["output"].as_str().ok_or("环境输出目录缺失")?);
    Ok(json!({"output":validate(root, &loaded["inputs"][3], output)?}))
}

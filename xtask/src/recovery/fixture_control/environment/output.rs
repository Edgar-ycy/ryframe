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
    reserve_service_root(root, &loaded["inputs"][1], output)?;
    Ok(json!({"output":validate(root, &loaded["inputs"][3], output)?}))
}

fn reserve_service_root(root: &Path, review: &Value, output: &Path) -> Result<()> {
    let future = review["future_root"].as_str().ok_or_else(|| {
        OutputError("审阅缺少 future_root 保留服务根；创建配置前已拒绝，未写入；请核对正式审阅并选择同级独立新目录".into())
    })?;
    validate_local_test_path(Path::new(future), root, LocalTestPathKind::StateDirectory)
        .map_err(|_| OutputError("审阅 future_root 必须是受控且无链接的保留服务根；创建配置前已拒绝，未写入；请核对正式审阅并选择同级独立新目录".into()))?;
    let output = comparison_path(output);
    let future = comparison_path(Path::new(future));
    if output.starts_with(&future) {
        return Err(OutputError("配置输出不得等于或位于 review.future_root 保留服务根内；本次在创建配置文件或输出目录前拒绝，未写入；请选择保留服务根的同级独立新目录".into()).into());
    }
    Ok(())
}

fn comparison_path(path: &Path) -> PathBuf {
    let path = super::super::portable_path(path);
    #[cfg(windows)]
    let path = PathBuf::from(path.as_os_str().to_ascii_lowercase());
    path.components().collect()
}

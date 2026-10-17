use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    super::{digest, json as json_io},
    common,
};
use crate::Result;

pub(crate) struct Initialized {
    pub(crate) initial: Value,
    pub(crate) request: Value,
    pub(crate) review: Value,
    pub(crate) selected: Value,
    pub(crate) root: PathBuf,
}

pub(crate) fn initialized(backend: &Path, binding: &Value) -> Result<Initialized> {
    let path = common::bound_file(backend, binding, "目标初始化收据")?;
    if path.file_name().and_then(|value| value.to_str()) != Some("initialized.json") {
        return Err("目标必须绑定 initialized.json 最终收据".into());
    }
    let root = path.parent().ok_or("初始化收据缺少父目录")?.to_path_buf();
    let initial = json_io::read(&path)?;
    if initial.get("status").and_then(Value::as_str) != Some("fresh_target_initialized") {
        return Err("目标初始化尚未完成".into());
    }
    let prepare_path = root.join("prepare.json");
    let prepare = json_io::read(&prepare_path)?;
    let prepare_digest = digest::file(&prepare_path)?;
    if initial.get("prepare_sha256").and_then(Value::as_str) != Some(prepare_digest.sha256.as_str())
    {
        return Err("目标初始化收据与 prepare.json 摘要不同".into());
    }
    let request = common::read_bound(
        backend,
        prepare.get("request").ok_or("prepare.json 缺少目标请求")?,
        "目标初始化请求",
    )?;
    if prepare.get("request_sha256").and_then(Value::as_str)
        != Some(digest::plan_hash(&request)?.as_str())
        || json_io::read(&root.join("request.json"))? != request
    {
        return Err("目标初始化请求、规范摘要或目录副本不同".into());
    }
    let review_value = request.get("review").ok_or("目标请求缺少审阅绑定")?;
    let review_binding = json_io::object(review_value, "目标审阅绑定")?;
    let mut review_descriptor = review_binding.clone();
    let canonical = review_descriptor
        .remove("canonical_sha256")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or("目标审阅绑定缺少规范摘要")?;
    let review_path =
        common::bound_file(backend, &Value::Object(review_descriptor), "目标审阅计划")?;
    let review = json_io::read(&review_path)?;
    if digest::plan_hash(&review)? != canonical {
        return Err("目标审阅计划规范摘要不同".into());
    }
    let selected = request
        .get("side")
        .and_then(Value::as_str)
        .and_then(|side| review.get("scopes").and_then(|value| value.get(side)))
        .cloned()
        .ok_or("目标审阅计划缺少所选侧")?;
    Ok(Initialized {
        initial,
        request,
        review,
        selected,
        root,
    })
}

pub(crate) fn runtime_directory(backend: &Path, selected: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(
        selected
            .get("runtime_dir")
            .and_then(Value::as_str)
            .ok_or("目标审阅缺少运行目录")?,
    );
    common::local_directory(backend, &path, "目标运行目录")
}

pub(crate) fn receipt_file(path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !digest::linked(&metadata) => {
            Ok(Some(json_io::read(path)?))
        }
        Ok(_) => Err(format!("目标运行收据必须是普通文件：{}", path.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取目标运行收据：{error}").into()),
    }
}

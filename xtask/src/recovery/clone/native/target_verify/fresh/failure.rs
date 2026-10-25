//! 阶段失败只公开类别；真实本地I/O属性保留给严格续作模型，不猜测传输错误。
use crate::Result;
use serde_json::{Value, json};
use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct Publication {
    pub(super) path: PathBuf,
    pub(super) cause: io::Error,
}
impl fmt::Display for Publication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "本地阶段收据发布失败；检查目标目录、文件独占及路径长度，保留现场"
        )
    }
}
impl Error for Publication {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

#[derive(Debug)]
pub(super) struct StageFailure {
    pub(super) category: &'static str,
}
impl fmt::Display for StageFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "受管阶段未完成（{}）；核对固定工具、配置与保留的私有诊断，禁止重放未知写入",
            self.category
        )
    }
}
impl Error for StageFailure {}

pub(super) fn publication(path: &Path, cause: io::Error) -> Box<dyn Error> {
    Box::new(Publication {
        path: path.to_owned(),
        cause,
    })
}
pub(super) fn category(error: &(dyn Error + 'static)) -> &'static str {
    if let Some(error) = error.downcast_ref::<StageFailure>() {
        return error.category;
    }
    let io = error
        .downcast_ref::<Publication>()
        .map(|e| &e.cause)
        .or_else(|| error.downcast_ref::<io::Error>());
    match io.map(io::Error::kind) {
        Some(io::ErrorKind::NotFound) => "FileNotFoundError",
        Some(io::ErrorKind::PermissionDenied) => "PermissionError",
        Some(_) => "OSError",
        None => "RustError",
    }
}
pub(super) fn os(error: &(dyn Error + 'static), receipt: Option<&Path>) -> Value {
    let Some(published) = error.downcast_ref::<Publication>() else {
        return Value::Null;
    };
    let file = &published.path;
    // 不返回原始错误Display、路径正文或配置值。
    json!({"errno":if published.cause.kind()==io::ErrorKind::NotFound {Some(2)} else {None},
        "winerror":if cfg!(windows){published.cause.raw_os_error()}else{None},
        "filename_role":if receipt==Some(file.as_path()){"command_receipt"}else{"other"},
        "filename_length":file.to_string_lossy().chars().count()})
}
pub(super) fn capture<T>(result: Result<T>) -> Result<T> {
    result.map_err(|_| {
        Box::new(StageFailure {
            category: "InventoryCaptureError",
        }) as Box<dyn Error>
    })
}

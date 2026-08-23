use std::path::{Path, PathBuf};

/// 返回后端 Cargo Workspace 根目录。
pub(crate) fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 必须位于 Cargo Workspace 根目录下")
        .to_path_buf()
}

/// 返回前后端并列检出时的默认前端目录。
pub(crate) fn default_frontend_dir() -> PathBuf {
    root_dir()
        .parent()
        .expect("后端 Workspace 必须具有父目录")
        .join("ryframe-vue3")
}

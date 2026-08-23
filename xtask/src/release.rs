use std::path::Path;

use crate::{Result, cli::ReleaseOptions, process::run, workspace::root_dir};

pub(crate) fn verify(options: &ReleaseOptions, frontend_dir: &Path) -> Result<()> {
    let frontend = frontend_dir
        .canonicalize()
        .map_err(|error| format!("无法解析前端目录：{error}"))?;
    let frontend = frontend.to_str().ok_or("前端目录路径不是有效 UTF-8")?;
    run(
        &root_dir(),
        "python",
        &[
            "scripts/validate_release.py",
            "--tag",
            &options.tag,
            "--frontend-dir",
            frontend,
            "--backend-repository",
            &options.backend_repository,
            "--backend-commit",
            &options.backend_commit,
            "--frontend-repository",
            &options.frontend_repository,
            "--frontend-commit",
            &options.frontend_commit,
            "--manifest-path",
            &options.manifest_path,
        ],
    )
}

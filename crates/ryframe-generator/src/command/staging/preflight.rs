use std::{path::Path, process::Command};

/// 复用正式组合根和 SDK 全量校验；关闭前端同步也不能安装冲突的业务模块。
pub(super) fn validate(backend: &Path, target: &Path, build_dir: &Path) -> Result<(), String> {
    println!("→ 核验全部已注册业务模块、权限与路由");
    let output = Command::new("cargo")
        .args([
            "run",
            "--locked",
            "--offline",
            "--no-default-features",
            "--features",
            "bin-api",
            "--manifest-path",
        ])
        .arg(backend.join("crates/ryframe/Cargo.toml"))
        .args(["--bin", "export_openapi", "--target-dir"])
        .arg(target)
        .arg("--")
        .arg(backend.join(".local-tests/business-module-preflight.json"))
        .current_dir(backend)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .output()
        .map_err(|error| format!("业务模块预检无法启动：{error}；请核对固定 Rust 工具链与 Cargo 可执行路径，再重新运行原预览命令；本批未安装"))?;
    if !output.status.success() {
        return Err(format!(
            "业务模块预检失败：{}\n{}\n请按上方 Cargo/SDK 诊断修正编译错误或模块的资源、表、路由、权限冲突，再重新运行原预览命令；若缺少锁定依赖缓存，先在原工作区执行 cargo fetch --locked。本批未安装",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

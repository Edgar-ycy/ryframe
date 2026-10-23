//! 编译时固化通用 Cargo 工作区来源清单，禁止运行时重算冒充编译输入。
#[path = "src/frontend_origin/sources.rs"]
mod sources;

const BUILD_SOURCE: &[u8] = include_bytes!("build.rs");
const HELPER_SOURCE: &[u8] = include_bytes!("src/frontend_origin/sources.rs");

fn main() -> std::process::ExitCode {
    match build() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn build() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").ok_or("缺少 Cargo 包目录")?,
    );
    verify_compiled_inputs(&manifest)?;
    let root = manifest
        .parent()
        .ok_or("缺少工作区根目录")?
        .canonicalize()?;
    let root = root.as_path();
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("缺少 Cargo 构建目录")?);
    let always_check = output.join("frontend-origin-always-check");
    match std::fs::symlink_metadata(&always_check) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(format!(
            "构建缓存不变量被破坏：{} 必须不存在（包括链接）；请核对并精确清理该 Cargo 缓存对象，或执行 cargo clean -p xtask --target-dir <本次目标目录>。不会自动删除未知对象。",
            always_check.display(),
        ).into()),
    }
    // Cargo 对不存在的监视路径每次重跑；此路径始终不生成。
    println!("cargo:rerun-if-changed={}", always_check.display());
    let paths = sources::discover(root)?;
    let mut entries = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed=../{path}");
        entries.push(serde_json::json!({"path":path,"sha256":hex::encode(Sha256::digest(std::fs::read(root.join(&path))?))}));
    }
    let rules: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(sources::POLICY))?)?;
    for tree in rules["trees"].as_array().ok_or("缺少来源目录")? {
        println!(
            "cargo:rerun-if-changed=../{}",
            tree.as_str().ok_or("来源目录无效")?
        );
    }
    write_if_changed(
        output.join("frontend-origin-sources.json"),
        serde_json::to_vec(&serde_json::json!({"root": root, "files": entries}))?,
    )?;
    // 记录真实构建脚本程序，回归测试复用同一程序核对不同 Cargo 运行目录。
    let program = std::env::current_exe()?;
    write_if_changed(
        output.join("frontend-origin-build-program.json"),
        serde_json::to_vec(&serde_json::json!({
            "path": program,
            "sha256": hex::encode(Sha256::digest(std::fs::read(&program)?)),
            "source_sha256": hex::encode(Sha256::digest(BUILD_SOURCE)),
            "helper_sha256": hex::encode(Sha256::digest(HELPER_SOURCE)),
        }))?,
    )?;
    Ok(())
}

// 同根同内容保持 mtime；替换只发生在完整内容写入并同步之后。
fn write_if_changed(path: std::path::PathBuf, bytes: Vec<u8>) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::read(&path) {
        Ok(current) if current == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

// 固化输入证明来自当前程序的编译，而非对当前文件重新计算后冒充。
fn verify_compiled_inputs(manifest: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    for (relative, compiled) in [
        ("build.rs", BUILD_SOURCE),
        ("src/frontend_origin/sources.rs", HELPER_SOURCE),
    ] {
        let path = manifest.join(relative);
        if std::fs::read(&path)? != compiled {
            return Err(format!(
                "构建脚本编译来源不匹配：{}；请执行 cargo clean -p xtask --target-dir <本次目标目录> 后从当前 Cargo 根重建。禁止以当前文件冒充程序编译输入。",
                path.display(),
            ).into());
        }
    }
    Ok(())
}

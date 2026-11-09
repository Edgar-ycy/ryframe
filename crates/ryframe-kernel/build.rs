use std::{
    env, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

mod build_support;

const DEVELOPMENT: &str = "development";

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo 必须提供输出目录"));
    let always_check = output.join("kernel-build-source-always-check");
    match fs::symlink_metadata(&always_check) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!(
            "无法核对构建缓存监视路径 {}：{error}",
            always_check.display()
        ),
        Ok(_) => panic!(
            "构建缓存不变量被破坏：{} 必须不存在（包括链接）；请核对并精确清理该 Cargo 缓存对象，或执行 cargo clean -p ryframe-kernel --target-dir <本次目标目录>。不会自动删除未知对象。",
            always_check.display()
        ),
    }
    // 不生成此监视路径，让 Cargo 每次执行时重新核对实际工作区的来源。
    println!("cargo:rerun-if-changed={}", always_check.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo 必须提供清单目录"));
    let workspace = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("ryframe-kernel 必须位于 Workspace crates 目录");
    let source_directory = workspace.join(".local-tests/build");
    fs::create_dir_all(&source_directory).unwrap_or_else(|error| {
        panic!(
            "无法准备构建来源目录 {}：{error}",
            source_directory.display()
        )
    });
    println!("cargo:rerun-if-changed={}", source_directory.display());
    let source = source_directory.join("source-commit");
    let commit = match fs::read(&source) {
        Ok(bytes) => build_support::parse_commit(&bytes).unwrap_or_else(|error| {
            panic!(
                "构建来源文件 {} 无效：{error}；请由正式构建入口重新生成",
                source.display()
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => DEVELOPMENT.to_owned(),
        Err(error) => panic!("无法读取构建来源文件 {}：{error}", source.display()),
    };
    write_if_changed(
        &output.join("build_source.rs"),
        format!("/// 编译时绑定的后端源码提交。\npub const BUILD_COMMIT: &str = {commit:?};\n")
            .as_bytes(),
    )
    .expect("写入构建来源常量失败");
}

// 同根同内容保持 mtime，变化时先持久化完整内容，再替换本次生成文件。
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match fs::read(path) {
        Ok(current) if current == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

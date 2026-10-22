use std::{env, fs, io::ErrorKind, path::PathBuf};

mod build_support;

const DEVELOPMENT: &str = "development";

fn main() {
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
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo 必须提供输出目录"))
        .join("build_source.rs");
    fs::write(
        output,
        format!("/// 编译时绑定的后端源码提交。\npub const BUILD_COMMIT: &str = {commit:?};\n"),
    )
    .expect("写入构建来源常量失败");
}

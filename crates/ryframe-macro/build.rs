use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn main() {
    if let Err(error) = build() {
        panic!("过程宏来源核验失败：{error}");
    }
}

fn build() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("缺少 Cargo 包目录")?);
    let output = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("缺少 Cargo 输出目录")?);
    let sentinel = output.join("macro-source-always-check");
    match fs::symlink_metadata(&sentinel) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(format!(
            "{} 必须不存在（包括链接）；请核对并精确清理该 Cargo 缓存对象，不会自动删除未知对象",
            sentinel.display()
        )
        .into()),
    }
    println!("cargo:rerun-if-changed={}", sentinel.display());
    // 脚本可能跨工作树复用；不能用当前文件冒充该程序实际编译的脚本。
    if fs::read(root.join("build.rs"))? != include_bytes!("build.rs") {
        return Err(
            "构建脚本编译输入不匹配；请仅清理 ryframe-macro 的当前 Cargo 缓存后重建".into(),
        );
    }
    let mut files = vec!["Cargo.toml".to_owned(), "build.rs".to_owned()];
    discover(&root, &root.join("src"), &mut files)?;
    files.sort();
    let mut hash = Sha256::new();
    for relative in files {
        println!("cargo:rerun-if-changed={relative}");
        let bytes = fs::read(root.join(&relative))?;
        hash.update((relative.len() as u64).to_be_bytes());
        hash.update(relative.as_bytes());
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(Sha256::digest(bytes));
    }
    println!("cargo:rerun-if-changed=src");
    let fingerprint = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    write_if_changed(
        &output.join("macro-source.sha256"),
        format!("{fingerprint}\n").as_bytes(),
    )?;
    Ok(())
}

fn discover(root: &Path, directory: &Path, files: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other("过程宏源码不允许链接"));
        }
        if metadata.is_dir() {
            discover(root, &entry.path(), files)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(std::io::Error::other)?
                    .to_str()
                    .ok_or_else(|| std::io::Error::other("源码路径不是 UTF-8"))?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match fs::read(path) {
        Ok(current) if current == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
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
        let _ = fs::remove_file(temporary);
    }
    result
}

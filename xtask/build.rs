//! 编译时固化通用 Cargo 工作区来源清单，禁止运行时重算冒充编译输入。
#[path = "src/frontend_origin/sources.rs"]
mod sources;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("缺少工作区根目录")?;
    let paths = sources::discover(root)?;
    let mut entries = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", root.join(&path).display());
        entries.push(serde_json::json!({"path":path,"sha256":hex::encode(Sha256::digest(std::fs::read(root.join(&path))?))}));
    }
    let rules: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(sources::POLICY))?)?;
    for tree in rules["trees"].as_array().ok_or("缺少来源目录")? {
        println!(
            "cargo:rerun-if-changed={}",
            root.join(tree.as_str().ok_or("来源目录无效")?).display()
        );
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("缺少 Cargo 构建目录")?);
    std::fs::write(
        output.join("frontend-origin-sources.json"),
        serde_json::to_vec(&entries)?,
    )?;
    Ok(())
}

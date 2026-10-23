//! 检查 Git 管理范围内的后端自维护源码，第三方 vendor 单独保留。

use std::path::Path;

use crate::Result;

use super::language::{git, inventory};

pub(crate) fn check_backend_sources(root: &Path) -> Result<()> {
    let inventory = inventory::discover(root, &mut |arguments| git::invoke(root, arguments))?;
    let forbidden = inventory
        .eligible
        .iter()
        .filter(|path| !path.starts_with("vendor/"))
        .filter(|path| {
            Path::new(path)
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
                    )
                })
        })
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !forbidden.is_empty() {
        return Err(format!(
            "后端自维护源码禁止 JavaScript/TypeScript，发现：{}；请将后端工具改用 Rust，纯前端职责放入前端仓库",
            forbidden.join("、")
        )
        .into());
    }
    Ok(())
}

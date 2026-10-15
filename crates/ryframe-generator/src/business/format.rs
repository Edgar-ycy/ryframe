use std::path::Path;

use super::writer::BusinessAsset;
use crate::{AssetRoot, GeneratedAsset, ResourceError};

pub(super) fn format_assets(
    workspace: &Path,
    assets: &mut [BusinessAsset],
) -> Result<(), ResourceError> {
    let mut sources = assets
        .iter_mut()
        .map(|asset| GeneratedAsset {
            resource: asset.resource.clone(),
            root: AssetRoot::Backend,
            path: asset.path.clone(),
            content: std::mem::take(&mut asset.content),
        })
        .collect::<Vec<_>>();
    crate::resource::rust_assets_in_workspace(workspace, &mut sources)?;
    for source in &sources {
        if source.path.ends_with(".rs") && source.content.lines().count() > 500 {
            return Err(ResourceError::new(
                format!(
                    "生成 Rust 文件超过 500 行：{}（{} 行）；本批未写入",
                    source.path,
                    source.content.lines().count()
                ),
                "按资源职责拆分模型或将复杂行为放入手写扩展；不能压缩格式或跳过文件预算",
            )
            .with_resource(&source.resource)
            .with_file(&source.path));
        }
    }
    for (asset, source) in assets.iter_mut().zip(sources) {
        asset.content = source.content;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn formatted_generated_rust_over_budget_is_rejected_before_installation() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let content = (0..501)
            .map(|index| format!("pub const VALUE_{index}: u8 = 0;\n"))
            .collect::<String>();
        let mut assets = vec![super::BusinessAsset::new(
            "wide",
            "src/generated/wide.rs",
            content,
        )];
        let error = super::format_assets(&workspace, &mut assets)
            .unwrap_err()
            .to_string();
        for detail in ["500 行", "wide.rs", "本批未写入", "按资源职责拆分"] {
            assert!(error.contains(detail), "{error}");
        }
    }
}

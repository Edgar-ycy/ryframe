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
    for (asset, source) in assets.iter_mut().zip(sources) {
        asset.content = source.content;
    }
    Ok(())
}

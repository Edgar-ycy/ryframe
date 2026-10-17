use std::path::Path;

use super::ResourceError;

pub(super) fn stable(root: &Path) -> Result<bool, ResourceError> {
    ryframe_config::release_stage(root, "HEAD")
        .map(|stage| stage.stable)
        .map_err(|error| {
            ResourceError::new(
                format!("无法检查 Workspace 版本阶段：{error}"),
                "修正 Workspace 版本；稳定版生效后不得降级绕过保护",
            )
        })
}

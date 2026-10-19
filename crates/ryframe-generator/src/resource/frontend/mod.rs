use std::{collections::BTreeSet, path::Path};

use serde::Serialize;

use super::{GeneratedAsset, ResourceError, ResourceIr};

mod codec;
mod transport;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct RenderRequest<'a> {
    format_version: u8,
    generator_version: &'static str,
    aggregate: bool,
    resources: &'a [ResourceIr],
}

/// 调用前端仓库的纯渲染器；返回完整候选资产，不安装文件或修改 ownership。
pub fn render_frontend_assets(
    resources: &[ResourceIr],
    frontend: &Path,
    aggregate: bool,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    validate_names(resources)?;
    let input = serde_json::to_vec(&RenderRequest {
        format_version: 1,
        generator_version: crate::GENERATOR_VERSION,
        aggregate,
        resources,
    })
    .map_err(|error| failure(format!("无法序列化前端资源请求：{error}")))?;
    if input.len() > MAX_INPUT_BYTES {
        return Err(failure("前端资源请求超过 16 MiB；本批未安装"));
    }
    let output = transport::invoke(frontend, &input)?;
    codec::decode_assets(&output, resources, aggregate)
}

fn validate_names(resources: &[ResourceIr]) -> Result<(), ResourceError> {
    let mut names = BTreeSet::new();
    for resource in resources {
        let mut name = resource.name.bytes();
        if !name.next().is_some_and(|byte| byte.is_ascii_lowercase())
            || !name.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || !names.insert(&resource.name)
        {
            return Err(failure("前端资源名称不安全或重复；本批未安装"));
        }
    }
    Ok(())
}

fn failure(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "核对前后端生成器版本、资源清单和 workspace.frontend_dir；先运行 corepack pnpm install --frozen-lockfile，再重新预览；不要手工修改生成文件",
    )
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    marker::PhantomData,
};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};

use crate::resource::{AssetRoot, GeneratedAsset, ResourceError, ResourceIr};

const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const CATALOG_PATH: &str = "src/generated/resources/index.ts";
const REQUIRED_FILES: [&str; 5] = [
    "api.ts",
    "page.vue",
    "registration.ts",
    "index.ts",
    "fields.ts",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format_version: u32,
    generator_version: String,
    assets: Vec<Object<WireAsset>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAsset {
    resource: String,
    root: String,
    path: String,
    content: String,
}

// 结构体派生反序列化也接受序列形式；协议只允许具名字段的 JSON 对象。
#[derive(Deserialize)]
#[serde(transparent, bound(deserialize = "T: Deserialize<'de>"))]
struct Object<T>(#[serde(deserialize_with = "deserialize_object")] T);

fn deserialize_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ObjectVisitor<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("JSON 对象")
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(ObjectVisitor(PhantomData))
}

/// 校验前端渲染协议，全部资产通过后才交给受管写入流程。
pub(super) fn decode_assets(
    source: &[u8],
    resources: &[ResourceIr],
    aggregate: bool,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    if source.len() > MAX_OUTPUT_BYTES {
        return Err(ResourceError::new(
            "前端渲染输出超过 64 MiB",
            "拆分资源批次并重新生成，不要扩大协议输出上限",
        ));
    }
    let resources = resource_index(resources)?;
    let source = std::str::from_utf8(source).map_err(|_| {
        ResourceError::new(
            "前端渲染输出不是有效 UTF-8",
            "修复前端渲染器，使用 UTF-8 输出 JSON",
        )
    })?;
    // 不转发解析器诊断，避免未知字段或非法值将原始输出带入错误。
    let Object(envelope): Object<Envelope> = serde_json::from_str(source).map_err(|_| {
        ResourceError::new(
            "前端渲染输出不符合严格 JSON 协议",
            "检查字段类型、缺失字段、重复或未知字段及 JSON 尾随内容，修复渲染器后重试",
        )
    })?;
    if envelope.format_version != 1 || envelope.generator_version != crate::GENERATOR_VERSION {
        return Err(ResourceError::new(
            "前端渲染协议版本或生成器版本不匹配",
            "同步前后端生成工具，使用当前生成器版本和 format_version=1 重新生成",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut assets = Vec::with_capacity(envelope.assets.len());
    let mut catalog_count = 0;
    for Object(asset) in envelope.assets {
        let header = validate_identity(&asset, &resources, aggregate)?;
        validate_content(&asset, &header)?;
        if !paths.insert(asset.path.clone()) {
            return Err(ResourceError::new(
                "前端渲染资产路径重复",
                "修复渲染器，确保每个受管路径只输出一次",
            ));
        }
        catalog_count += usize::from(asset.resource == "__catalog__");
        assets.push(GeneratedAsset {
            resource: asset.resource,
            root: AssetRoot::Frontend,
            path: asset.path,
            content: asset.content,
        });
    }
    validate_completeness(&resources, &paths, catalog_count, aggregate)?;
    assets.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(assets)
}

fn resource_index(resources: &[ResourceIr]) -> Result<BTreeMap<&str, &ResourceIr>, ResourceError> {
    let mut index = BTreeMap::new();
    for resource in resources {
        if !resource
            .name
            .starts_with(|character: char| character.is_ascii_lowercase())
            || !resource
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ResourceError::new(
                "资源 IR 名称不是安全的小写 snake_case 标识",
                "修复资源清单，以小写字母开头且只使用小写字母、数字和下划线，再重新校验 IR",
            ));
        }
        if index.insert(resource.name.as_str(), resource).is_some() {
            return Err(ResourceError::new(
                "资源 IR 名称重复",
                "每个资源名称只保留一个已校验的资源 IR",
            ));
        }
    }
    Ok(index)
}

fn validate_identity(
    asset: &WireAsset,
    resources: &BTreeMap<&str, &ResourceIr>,
    aggregate: bool,
) -> Result<String, ResourceError> {
    if asset.root != "frontend" {
        return Err(ResourceError::new(
            "前端渲染器输出了非 frontend 根资产",
            "修复渲染器，仅输出 frontend 根目录内的受管资产",
        ));
    }
    if asset.resource == "__catalog__" {
        if !aggregate || asset.path != CATALOG_PATH {
            return Err(ResourceError::new(
                "前端聚合资产未获请求或路径不正确",
                "仅在 aggregate=true 时输出唯一的 src/generated/resources/index.ts 聚合资产",
            ));
        }
        return Ok(format!(
            "// @generated by RyFrame {}\n// 请勿手工修改；修改资源清单后重新生成。\n\n",
            crate::GENERATOR_VERSION,
        ));
    }
    let resource = resources.get(asset.resource.as_str()).ok_or_else(|| {
        ResourceError::new(
            "前端资产引用了未知资源",
            "仅输出本次请求所含资源的资产，检查资源归属",
        )
    })?;
    let prefix = format!("src/generated/resources/{}/", resource.name);
    let relative = asset.path.strip_prefix(&prefix).ok_or_else(path_error)?;
    validate_relative_path(relative)?;
    if relative == "page.vue" {
        Ok(format!(
            "<!-- @generated by RyFrame {} | resource: {} | source: {} -->\n<!-- 请勿手工修改；修改资源清单后重新生成。 -->\n",
            crate::GENERATOR_VERSION,
            resource.name,
            resource.source_path,
        ))
    } else {
        Ok(format!(
            "// @generated by RyFrame {}\n// resource: {}\n// source: {}\n// 请勿手工修改；修改资源清单后重新生成。\n\n",
            crate::GENERATOR_VERSION,
            resource.name,
            resource.source_path,
        ))
    }
}

fn validate_relative_path(relative: &str) -> Result<(), ResourceError> {
    let module = if relative == "page.vue" {
        "page"
    } else {
        relative.strip_suffix(".ts").ok_or_else(path_error)?
    };
    for segment in module.split('/') {
        if segment.is_empty()
            || !segment.starts_with(|character: char| character.is_ascii_alphanumeric())
            || !segment.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return Err(path_error());
        }
    }
    Ok(())
}

fn path_error() -> ResourceError {
    ResourceError::new(
        "前端资产路径不安全或超出允许范围",
        "仅在对应资源目录输出 page.vue 或 .ts 模块；路径段使用小写字母、数字、下划线或连字符，禁止空段、点段、反斜杠及绝对路径",
    )
}

fn validate_content(asset: &WireAsset, header: &str) -> Result<(), ResourceError> {
    if asset.content.contains('\r') {
        return Err(ResourceError::new(
            "前端生成内容包含 CR 换行",
            "修复渲染器，统一使用 LF 换行后重新生成",
        ));
    }
    if !asset.content.starts_with(header) {
        return Err(ResourceError::new(
            "前端资产缺少精确的生成器、资源或来源头",
            "使用当前生成器版本及请求 IR 的资源名称和 source_path 输出完整受管文件头；聚合资产使用聚合头",
        ));
    }
    let max_lines = if asset.path.ends_with(".vue") {
        600
    } else {
        300
    };
    if asset.content.lines().count() > max_lines {
        return Err(ResourceError::new(
            format!("前端生成文件超过 {max_lines} 行"),
            "按职责拆分 TypeScript 模块或 Vue 页面后重新生成，不要扩大单文件上限",
        ));
    }
    Ok(())
}

fn validate_completeness(
    resources: &BTreeMap<&str, &ResourceIr>,
    paths: &BTreeSet<String>,
    catalog_count: usize,
    aggregate: bool,
) -> Result<(), ResourceError> {
    for name in resources.keys() {
        for file in REQUIRED_FILES {
            if !paths.contains(&format!("src/generated/resources/{name}/{file}")) {
                return Err(ResourceError::new(
                    "前端渲染结果缺少资源必需资产",
                    "为每个请求资源完整输出 api.ts、page.vue、registration.ts、index.ts 和 fields.ts",
                ));
            }
        }
    }
    if catalog_count != usize::from(aggregate) {
        return Err(ResourceError::new(
            "前端渲染结果缺少唯一聚合资产",
            "aggregate=true 时输出恰好一个 __catalog__ 的 src/generated/resources/index.ts 资产",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/internal/frontend_protocol.rs"]
mod tests;

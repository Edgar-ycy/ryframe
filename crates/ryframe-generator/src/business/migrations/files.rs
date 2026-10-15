use std::fs;

use serde::Deserialize;

use super::{History, ResourceHistory, Version, error, hash};
use crate::{
    ResourceError,
    business::{BusinessPackage, OwnedResourceDescriptor, model::descriptor_to_ir},
};

pub(super) fn load(package: &BusinessPackage) -> Result<History, ResourceError> {
    let path = package.root.join(super::HISTORY_PATH);
    match fs::read_to_string(&path) {
        Ok(source) => {
            let history: History = serde_json::from_str(&source)
                .map_err(|cause| error(format!("迁移历史格式错误：{cause}")))?;
            super::validate(&history, &package.module)?;
            Ok(history)
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => existing_initials(package),
        Err(cause) => Err(error(format!("读取迁移历史失败：{cause}"))),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousDescriptors {
    format_version: u16,
    module: String,
    resources: Vec<OwnedResourceDescriptor>,
}

// 开发版已有受管初始迁移只能从已核验的生成描述及注册表建立历史，不能读取数据库猜测。
fn existing_initials(package: &BusinessPackage) -> Result<History, ResourceError> {
    let path = package.root.join("src/generated/resources.json");
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Ok(History {
                format_version: 1,
                module: package.module.clone(),
                resources: Vec::new(),
            });
        }
        Err(cause) => return Err(error(format!("读取已有资源描述失败：{cause}"))),
    };
    let previous: PreviousDescriptors = serde_json::from_str(&source)
        .map_err(|cause| error(format!("已有资源描述格式错误：{cause}")))?;
    if previous.format_version != 1 || previous.module != package.module {
        return Err(error("已有资源描述版本或归属不匹配"));
    }
    let registry = fs::read_to_string(package.root.join("src/generated/migrations/mod.rs"))
        .map_err(|cause| error(format!("读取已有迁移注册表失败：{cause}")))?;
    let mut resources = Vec::new();
    for descriptor in previous.resources {
        let resource = descriptor_to_ir(package, &descriptor)?;
        let id = format!("m_resource_initial_{}", resource.name);
        let source = fs::read(package.root.join(format!("migrations/{id}.rs")))
            .map_err(|cause| error(format!("读取已有初始迁移失败：{cause}")))?;
        let checksum = hash(&source);
        let canonical = crate::resource::canonical_schema(&resource);
        if !registry.contains(&checksum)
            || !registry.contains(&format!("{:?}", resource.schema_hash))
            || !registry.contains(&format!("{canonical:?}"))
        {
            return Err(error(format!(
                "资源 {} 的已有初始迁移与注册结构不一致，不能自动建立历史",
                resource.name
            )));
        }
        resources.push(ResourceHistory {
            name: resource.name,
            descriptor,
            versions: vec![Version {
                id,
                checksum,
                schema_hash: resource.schema_hash,
                canonical,
            }],
        });
    }
    let history = History {
        format_version: 1,
        module: package.module.clone(),
        resources,
    };
    super::validate(&history, &package.module)?;
    Ok(history)
}

pub(super) fn migration(
    package: &BusinessPackage,
    version: &Version,
) -> Result<String, ResourceError> {
    let path = package.root.join(format!("migrations/{}.rs", version.id));
    let source = fs::read_to_string(&path)
        .map_err(|cause| error(format!("历史迁移 {} 读取失败：{cause}", version.id)))?;
    if hash(source.as_bytes()) != version.checksum {
        return Err(error(format!(
            "历史迁移 {} 摘要不符，禁止覆盖或补签",
            version.id
        )));
    }
    Ok(source)
}

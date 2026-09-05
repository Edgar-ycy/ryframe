use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::{
    GeneratedAsset, GeneratedCatalog, MANIFEST_PATH, OwnershipEntry, OwnershipManifest,
    ResourceError, ResourceIr, ResourceWorkspace, content_hash, target_path, validate_managed_path,
};

pub(super) fn validate_workspace(workspace: ResourceWorkspace<'_>) -> Result<(), ResourceError> {
    if !workspace.backend_root.is_dir() {
        return Err(ResourceError::file(
            workspace.backend_root,
            "后端工作区不存在或不是目录",
            "传入包含 Cargo.toml 的后端仓库根目录",
        ));
    }
    if !workspace.backend_root.join("Cargo.toml").is_file() {
        return Err(ResourceError::file(
            workspace.backend_root,
            "后端工作区缺少 Cargo.toml",
            "传入后端 Cargo Workspace 根目录，而不是其父目录或子目录",
        ));
    }
    if let Some(frontend) = workspace.frontend_root
        && !frontend.is_dir()
    {
        return Err(ResourceError::file(
            frontend,
            "前端工作区不存在或不是目录",
            "传入前端仓库根目录",
        ));
    }
    Ok(())
}

pub(super) fn load_manifest(path: &Path) -> Result<OwnershipManifest, ResourceError> {
    load_manifest_snapshot(path).map(|(manifest, _)| manifest)
}

pub(super) fn verify_unselected_resource_sources(
    catalog: &GeneratedCatalog,
    selected: &str,
    manifest: &OwnershipManifest,
) -> Result<(), ResourceError> {
    let mut managed_hashes = BTreeMap::<&str, BTreeSet<&str>>::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != selected && entry.resource != "__catalog__")
    {
        managed_hashes
            .entry(&entry.resource)
            .or_default()
            .insert(&entry.source_hash);
    }
    for (resource, hashes) in managed_hashes {
        if hashes.len() != 1 {
            return Err(ResourceError::new(
                "同一受管资源记录了不一致的清单哈希",
                "从版本控制恢复该资源全部生成文件与 ownership manifest 后重新生成",
            )
            .with_resource(resource)
            .with_file(MANIFEST_PATH));
        }
        let current = catalog.resources.get(resource).ok_or_else(|| {
            ResourceError::new(
                "未选中的受管资源缺少当前清单，不能安全刷新聚合入口",
                format!("恢复 catalog/resources/{resource}.toml；移除资源应使用显式删除流程"),
            )
            .with_resource(resource)
            .with_file(MANIFEST_PATH)
        })?;
        let owned_hash = hashes
            .first()
            .expect("非空受管资源哈希集合已由 manifest 条目建立");
        if current.source_hash != *owned_hash {
            return Err(ResourceError::new(
                "未选中的受管资源清单已有待生成变化，拒绝提前刷新中央聚合",
                format!(
                    "先运行 cargo xtask generate resource {resource} --write，再重新生成所选资源 {selected}"
                ),
            )
            .with_resource(resource)
            .with_file(&current.source_path));
        }
    }
    Ok(())
}

pub(super) fn verify_no_resource_removal(
    catalog: &GeneratedCatalog,
    manifest: &OwnershipManifest,
) -> Result<(), ResourceError> {
    let missing = manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != "__catalog__")
        .map(|entry| entry.resource.as_str())
        .filter(|resource| !catalog.resources.contains_key(*resource))
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    let names = missing.into_iter().collect::<Vec<_>>().join(", ");
    Err(ResourceError::new(
        format!("当前 catalog 缺少已受管资源：{names}；拒绝把缺失清单推断为删除"),
        "当前版本尚不支持资源退役；恢复对应 TOML。后续显式 remove 流程必须保留不可变初始迁移并预览全部删除后才能执行",
    )
    .with_file(MANIFEST_PATH))
}

pub(super) fn load_manifest_snapshot(
    path: &Path,
) -> Result<(OwnershipManifest, Option<Vec<u8>>), ResourceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((OwnershipManifest::default(), None));
        }
        Err(error) => {
            return Err(ResourceError::file(
                path,
                format!("无法读取 ownership manifest：{error}"),
                "恢复该文件或删除全部受管生成目录后重新生成",
            ));
        }
    };
    let source = std::str::from_utf8(&bytes).map_err(|error| {
        ResourceError::file(
            path,
            format!("ownership manifest 不是 UTF-8：{error}"),
            "从版本控制恢复 manifest，不要手工编辑",
        )
    })?;
    let manifest = toml::from_str::<OwnershipManifest>(source).map_err(|error| {
        ResourceError::file(
            path,
            format!("ownership manifest 格式错误：{error}"),
            "从版本控制恢复 manifest，不要手工编辑",
        )
    })?;
    if manifest.format_version != 1 {
        return Err(ResourceError::file(
            path,
            format!(
                "不支持 ownership format_version {}",
                manifest.format_version
            ),
            "使用匹配项目版本的生成器重新生成",
        ));
    }
    let mut paths = BTreeSet::new();
    for entry in &manifest.entries {
        validate_managed_path(entry.root, &entry.path)?;
        if !paths.insert((entry.root, entry.path.as_str())) {
            return Err(ResourceError::file(
                path,
                format!("ownership 中路径 `{}` 重复", entry.path),
                "从版本控制恢复 manifest，不要手工合并条目",
            ));
        }
    }
    Ok((manifest, Some(bytes)))
}

pub(super) fn verify_owned_files(
    manifest: &OwnershipManifest,
    workspace: ResourceWorkspace<'_>,
) -> Result<(), ResourceError> {
    for entry in &manifest.entries {
        let path = target_path(workspace, entry.root, &entry.path)?;
        let bytes = fs::read(&path).map_err(|error| {
            ResourceError::new(
                format!("受管文件丢失或无法读取：{error}"),
                "从版本控制恢复文件，或同时恢复匹配的 ownership manifest",
            )
            .with_resource(&entry.resource)
            .with_file(path.to_string_lossy())
        })?;
        if content_hash(&bytes) != entry.content_hash {
            return Err(ResourceError::new(
                "检测到对生成文件的人工修改，已停止且未写入任何文件",
                "把改动迁移到资源清单或强类型扩展，再从版本控制恢复生成文件",
            )
            .with_resource(&entry.resource)
            .with_file(path.to_string_lossy()));
        }
    }
    Ok(())
}

pub(super) fn desired_entries(
    assets: &[GeneratedAsset],
    resources: &BTreeMap<String, ResourceIr>,
) -> Vec<OwnershipEntry> {
    let mut entries = assets
        .iter()
        .map(|asset| {
            let resource = resources.get(&asset.resource);
            OwnershipEntry {
                resource: asset.resource.clone(),
                root: asset.root,
                path: asset.path.clone(),
                source_hash: resource.map_or_else(
                    || content_hash(asset.content.as_bytes()),
                    |resource| resource.source_hash.clone(),
                ),
                schema_hash: resource.map(|resource| resource.schema_hash.clone()),
                schema_revision: resource.and_then(|resource| resource.schema_revision.clone()),
                content_hash: content_hash(asset.content.as_bytes()),
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    entries
}

pub(super) fn manifests_equal(old: &OwnershipManifest, desired: &[OwnershipEntry]) -> bool {
    old.format_version == 1
        && old.generator_version == crate::GENERATOR_VERSION
        && old.entries == desired
}

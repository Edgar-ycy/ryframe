use super::*;

pub(super) struct ChangeSet<'a> {
    pub changed: Vec<&'a GeneratedAsset>,
    pub obsolete: Vec<&'a super::super::OwnershipEntry>,
    pub created_paths: BTreeSet<(AssetRoot, String)>,
    pub expected_files: BTreeMap<(AssetRoot, String), ExpectedFile>,
    pub report: SafeWriteReport,
}

impl<'a> ChangeSet<'a> {
    pub(super) fn prepare(
        workspace: ResourceWorkspace<'_>,
        old_manifest: &'a OwnershipManifest,
        desired_assets: &'a [GeneratedAsset],
    ) -> Result<Self, ResourceError> {
        let old_by_path = old_manifest
            .entries
            .iter()
            .map(|entry| ((entry.root, entry.path.clone()), entry))
            .collect::<BTreeMap<_, _>>();
        let desired_by_path = desired_assets
            .iter()
            .map(|asset| ((asset.root, asset.path.clone()), asset))
            .collect::<BTreeMap<_, _>>();

        let mut report = SafeWriteReport::default();
        let mut changed = Vec::new();
        let mut created_paths = BTreeSet::new();
        for (key, asset) in &desired_by_path {
            let target = target_path(workspace, key.0, &key.1)?;
            let hash = content_hash(asset.content.as_bytes());
            match old_by_path.get(key) {
                Some(old) if old.content_hash == hash => {
                    report.unchanged.push(display_path(key.0, &key.1));
                }
                Some(_) => changed.push(*asset),
                None if target.exists() => {
                    return Err(ResourceError::new(
                        "目标文件已经存在，但不属于当前生成器",
                        "移动该文件，或确认内容后由人工删除；生成器不会覆盖未受管文件",
                    )
                    .with_resource(&asset.resource)
                    .with_file(target.to_string_lossy()));
                }
                None => {
                    created_paths.insert((key.0, key.1.clone()));
                    changed.push(*asset);
                }
            }
        }

        let obsolete = old_manifest
            .entries
            .iter()
            .filter(|entry| !desired_by_path.contains_key(&(entry.root, entry.path.clone())))
            .collect::<Vec<_>>();
        let affected = changed
            .iter()
            .map(|asset| (asset.root, asset.path.clone()))
            .chain(
                obsolete
                    .iter()
                    .map(|entry| (entry.root, entry.path.clone())),
            )
            .collect::<BTreeSet<_>>();
        let expected_files = affected
            .iter()
            .map(|key| {
                let expected = old_by_path.get(key).map_or(ExpectedFile::Absent, |entry| {
                    ExpectedFile::ContentHash(entry.content_hash.clone())
                });
                (key.clone(), expected)
            })
            .collect::<BTreeMap<_, _>>();
        Ok(Self {
            changed,
            obsolete,
            created_paths,
            expected_files,
            report,
        })
    }
}

use super::*;

pub(super) struct StagedWrite {
    backend_stage: tempfile::TempDir,
    frontend_stage: Option<tempfile::TempDir>,
    manifest_content: String,
}

#[derive(Default)]
struct InstallJournal {
    backups: Vec<(PathBuf, PathBuf)>,
    installed: Vec<InstalledFile>,
}

impl StagedWrite {
    pub(super) fn prepare(
        workspace: ResourceWorkspace<'_>,
        changes: &ChangeSet<'_>,
        entries: Vec<super::super::OwnershipEntry>,
    ) -> Result<Self, ResourceError> {
        let backend_stage = tempfile::Builder::new()
            .prefix(".ryframe-generator-stage-")
            .tempdir_in(workspace.backend_root)
            .map_err(|error| {
                ResourceError::file(
                    workspace.backend_root,
                    format!("无法创建后端临时生成目录：{error}"),
                    "确认工作区可写且磁盘空间充足",
                )
            })?;
        let touches_frontend = changes
            .changed
            .iter()
            .any(|asset| asset.root == AssetRoot::Frontend)
            || changes
                .obsolete
                .iter()
                .any(|entry| entry.root == AssetRoot::Frontend);
        let frontend_stage = if touches_frontend {
            let root = workspace.frontend_root.ok_or_else(|| {
                ResourceError::new(
                    "生成结果包含前端资产，但没有提供前端工作区",
                    "为 ResourceWorkspace.frontend_root 提供前端仓库路径",
                )
            })?;
            Some(
                tempfile::Builder::new()
                    .prefix(".ryframe-generator-stage-")
                    .tempdir_in(root)
                    .map_err(|error| {
                        ResourceError::file(
                            root,
                            format!("无法创建前端临时生成目录：{error}"),
                            "确认前端工作区可写且磁盘空间充足",
                        )
                    })?,
            )
        } else {
            None
        };

        for asset in &changes.changed {
            let stage_root = match asset.root {
                AssetRoot::Backend => backend_stage.path(),
                AssetRoot::Frontend => frontend_stage
                    .as_ref()
                    .expect("前端资产已创建对应临时目录")
                    .path(),
            };
            let staged = stage_root.join(&asset.path);
            write_staged(&staged, &asset.content, &asset.resource)?;
        }

        let new_manifest = OwnershipManifest {
            format_version: 1,
            generator_version: crate::GENERATOR_VERSION.into(),
            entries,
        };
        let manifest_content = toml::to_string_pretty(&new_manifest).map_err(|error| {
            ResourceError::new(
                format!("无法序列化 ownership manifest：{error}"),
                "检查 manifest 数据是否只包含稳定基础类型",
            )
        })?;
        let staged_manifest = backend_stage.path().join(MANIFEST_PATH);
        write_staged(&staged_manifest, &manifest_content, "__catalog__")?;

        let backend_backup = backend_stage.path().join(".backup");
        fs::create_dir_all(&backend_backup).map_err(|error| {
            ResourceError::file(
                &backend_backup,
                format!("无法创建回滚目录：{error}"),
                "确认工作区可写且未被安全软件锁定",
            )
        })?;
        if let Some(stage) = &frontend_stage {
            let frontend_backup = stage.path().join(".backup");
            fs::create_dir_all(&frontend_backup).map_err(|error| {
                ResourceError::file(
                    &frontend_backup,
                    format!("无法创建前端回滚目录：{error}"),
                    "确认前端工作区可写且未被安全软件锁定",
                )
            })?;
        }

        Ok(Self {
            backend_stage,
            frontend_stage,
            manifest_content,
        })
    }

    pub(super) fn commit(
        self,
        workspace: ResourceWorkspace<'_>,
        old_manifest: &OwnershipManifest,
        expected_manifest: ExpectedFile,
        changes: &mut ChangeSet<'_>,
    ) -> Result<(), ResourceError> {
        let mut journal = InstallJournal::default();
        let transaction = self.install(
            workspace,
            old_manifest,
            &expected_manifest,
            changes,
            &mut journal,
        );
        if let Err(error) = transaction {
            return match rollback(&journal.installed, &journal.backups) {
                Ok(()) => Err(error),
                Err(rollback_error) => {
                    let recovery_directories =
                        persist_recovery_directories(self.backend_stage, self.frontend_stage);
                    let recovery_paths = recovery_directories
                        .iter()
                        .map(|path| path.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("；");
                    Err(ResourceError::new(
                        format!("写入事务失败：{error}；回滚也未完整完成：{rollback_error}"),
                        format!(
                            "停止继续生成；持久化备份目录为 {recovery_paths}；按错误中的目标与备份路径人工恢复，恢复前不要删除这些目录或 ownership manifest"
                        ),
                    ))
                }
            };
        }
        Ok(())
    }

    fn install(
        &self,
        workspace: ResourceWorkspace<'_>,
        old_manifest: &OwnershipManifest,
        expected_manifest: &ExpectedFile,
        changes: &mut ChangeSet<'_>,
        journal: &mut InstallJournal,
    ) -> Result<(), ResourceError> {
        let Self {
            backend_stage,
            frontend_stage,
            manifest_content,
        } = self;
        let manifest_path = workspace.backend_root.join(MANIFEST_PATH);
        let staged_manifest = backend_stage.path().join(MANIFEST_PATH);
        let backend_backup = backend_stage.path().join(".backup");
        for ((root, path), expected) in &changes.expected_files {
            let target = target_path(workspace, *root, path)?;
            verify_expected_file(&target, expected)?;
            if expected.exists() {
                let backup = match root {
                    AssetRoot::Backend => backend_backup.join(path),
                    AssetRoot::Frontend => frontend_stage
                        .as_ref()
                        .expect("前端事务必须有同卷临时目录")
                        .path()
                        .join(".backup")
                        .join(path),
                };
                move_to_backup(&target, &backup)?;
                journal.backups.push((target, backup));
            }
        }
        verify_expected_file(&manifest_path, expected_manifest)?;
        if expected_manifest.exists() {
            let backup = backend_backup.join("ownership.toml");
            move_to_backup(&manifest_path, &backup)?;
            journal.backups.push((manifest_path.clone(), backup));
        }

        for asset in &changes.changed {
            let stage_root = match asset.root {
                AssetRoot::Backend => backend_stage.path(),
                AssetRoot::Frontend => frontend_stage
                    .as_ref()
                    .expect("前端临时目录必须存在")
                    .path(),
            };
            let staged = stage_root.join(&asset.path);
            let target = target_path(workspace, asset.root, &asset.path)?;
            verify_expected_file(&target, &ExpectedFile::Absent)?;
            install_staged(&staged, &target, &asset.resource)?;
            journal.installed.push(InstalledFile {
                path: target,
                content_hash: content_hash(asset.content.as_bytes()),
            });
            let display = display_path(asset.root, &asset.path);
            if changes
                .created_paths
                .contains(&(asset.root, asset.path.clone()))
            {
                changes.report.created.push(display.clone());
            } else {
                changes.report.updated.push(display.clone());
            }
            changes.report.written.push(display);
        }
        for entry in old_manifest.entries.iter().filter(|entry| {
            !changes
                .expected_files
                .contains_key(&(entry.root, entry.path.clone()))
        }) {
            let target = target_path(workspace, entry.root, &entry.path)?;
            verify_expected_file(
                &target,
                &ExpectedFile::ContentHash(entry.content_hash.clone()),
            )?;
        }
        verify_expected_file(&manifest_path, &ExpectedFile::Absent)?;
        install_staged(&staged_manifest, &manifest_path, "__catalog__")?;
        journal.installed.push(InstalledFile {
            path: manifest_path,
            content_hash: content_hash(manifest_content.as_bytes()),
        });
        Ok(())
    }
}

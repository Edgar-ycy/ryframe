use std::path::{Path, PathBuf};

use super::{
    BusinessBootstrapReport, add_composition_dependency, add_module_registration,
    add_workspace_member, business_lib, business_manifest, empty_generated_module, error,
    format_planned, ownership_manifest, read, resources_module,
};
use crate::resource::write_transaction::ExpectedFile;
use sha2::{Digest, Sha256};

pub(super) struct PlannedFile {
    pub(super) path: PathBuf,
    pub(super) content: String,
    pub(super) expected: ExpectedFile,
}

impl PlannedFile {
    pub(super) fn changed(&self) -> bool {
        match &self.expected {
            ExpectedFile::Absent => true,
            ExpectedFile::ExactBytes(bytes) => bytes != self.content.as_bytes(),
            ExpectedFile::ContentHash(hash) => {
                *hash != hex::encode(Sha256::digest(self.content.as_bytes()))
            }
        }
    }
}

pub(super) fn build(root: &Path, module: &str) -> Result<Vec<PlannedFile>, crate::ResourceError> {
    let package = format!("{module}-business");
    let crate_root = root.join("crates").join(&package);
    if crate_root.exists() {
        return Err(error(
            format!("业务 crate 已存在：{}", crate_root.display()),
            &crate_root,
        ));
    }
    let root_manifest = root.join("Cargo.toml");
    let composition_manifest = root.join("crates/ryframe/Cargo.toml");
    let registry = root.join("crates/ryframe/src/business.rs");
    let root_source = read(&root_manifest)?;
    let composition_source = read(&composition_manifest)?;
    let registry_source = read(&registry)?;
    let generated = empty_generated_module();
    let mut files = [
        ("Cargo.toml", business_manifest(&package, module)),
        ("src/lib.rs", business_lib()),
        ("src/resources/mod.rs", resources_module()),
        (
            "src/extensions/mod.rs",
            "// 在此实现标准 CRUD 无法表达的领域流程。\n".into(),
        ),
        ("src/generated/mod.rs", generated.clone()),
        (".ryframe/generated.toml", ownership_manifest(&generated)),
    ]
    .into_iter()
    .map(|(path, content)| PlannedFile {
        path: crate_root.join(path),
        content,
        expected: ExpectedFile::Absent,
    })
    .collect::<Vec<_>>();
    files.extend([
        PlannedFile {
            path: root_manifest,
            content: add_workspace_member(&root_source.replace("\r\n", "\n"), &package)?,
            expected: ExpectedFile::ExactBytes(root_source.into_bytes()),
        },
        PlannedFile {
            path: composition_manifest,
            content: add_composition_dependency(
                &composition_source.replace("\r\n", "\n"),
                &package,
            )?,
            expected: ExpectedFile::ExactBytes(composition_source.into_bytes()),
        },
        PlannedFile {
            path: registry,
            content: add_module_registration(&registry_source, &package.replace('-', "_"))?,
            expected: ExpectedFile::ExactBytes(registry_source.into_bytes()),
        },
    ]);
    format_planned(root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub(super) fn report(
    root: &Path,
    files: &[PlannedFile],
) -> Result<BusinessBootstrapReport, crate::ResourceError> {
    let mut report = BusinessBootstrapReport::default();
    for file in files {
        let path = relative_path(root, &file.path)?;
        if !file.expected.exists() {
            report.created.push(path);
        } else if file.changed() {
            report.updated.push(path);
        }
    }
    Ok(report)
}

pub(super) fn install(root: &Path, files: &[PlannedFile]) -> Result<(), crate::ResourceError> {
    use crate::{
        business::writer::{BusinessAsset, staged::apply_files},
        resource::write_transaction::verify_expected_file,
    };
    let mut assets = Vec::new();
    let mut expected = std::collections::BTreeMap::new();
    for file in files {
        verify_expected_file(&file.path, &file.expected)?;
        let relative = relative_path(root, &file.path)?;
        expected.insert(relative.clone(), file.expected.clone());
        if file.changed() {
            assets.push(BusinessAsset::new(
                "__bootstrap__",
                relative,
                file.content.clone(),
            ));
        }
    }
    // 归属清单最后安装，避免它先于所声明的生成文件出现。
    assets.sort_by_key(|asset| {
        if asset.path.ends_with("/.ryframe/generated.toml") {
            2
        } else {
            u8::from(expected[&asset.path].exists())
        }
    });
    apply_files(root, &assets, &[], &expected)
}

fn relative_path(root: &Path, path: &Path) -> Result<String, crate::ResourceError> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| error("创建路径超出 Workspace", path))
}

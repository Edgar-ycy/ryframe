use super::*;

pub(super) fn parse_local_patch_policy(value: Option<&Value>) -> PolicyResult<Vec<LocalPatch>> {
    let entries = array(value, "local_patch_licenses")?;
    let mut seen_patches = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut patches = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let label = format!("本地 patch 许可证[{index}]");
        let patch = object(Some(value), &label)?;
        exact_keys(
            patch,
            &[
                "patch",
                "package",
                "version",
                "path",
                "license_expression",
                "license_files",
            ],
            &label,
        )?;
        let patch_name = text(patch.get("patch"), &format!("{label}.patch"), 1)?;
        let package = text(patch.get("package"), &format!("{label}.package"), 1)?;
        let version = text(patch.get("version"), &format!("{label}.version"), 1)?;
        if !package_name(&patch_name) {
            return Err(format!("{label}.patch 不是合法 Cargo 包名"));
        }
        if !package_name(&package) {
            return Err(format!("{label}.package 不是合法 Cargo 包名"));
        }
        if !semver(&version) {
            return Err(format!("{label}.version 必须固定到完整三段版本"));
        }
        let path = portable_path(patch.get("path"), &format!("{label}.path"))?;
        let patch_identity = patch_name.to_lowercase();
        let path_identity = path.to_lowercase();
        if !seen_patches.insert(patch_identity) || !seen_paths.insert(path_identity) {
            return Err(format!("{label} 重复声明 patch 或路径"));
        }
        let license_expression = text(
            patch.get("license_expression"),
            &format!("{label}.license_expression"),
            1,
        )?;
        let declared =
            license_identifiers(&license_expression, &format!("{label}.license_expression"))?
                .into_iter()
                .collect::<BTreeSet<_>>();
        let files = array(
            patch.get("license_files"),
            &format!("{label}.license_files"),
        )?;
        if files.is_empty() {
            return Err(format!("{label}.license_files 不得为空"));
        }
        let mut seen_spdx = BTreeSet::new();
        let mut seen_files = BTreeSet::new();
        let mut parsed_files = Vec::with_capacity(files.len());
        for (file_index, value) in files.iter().enumerate() {
            let file_label = format!("{label}.license_files[{file_index}]");
            let license_file = object(Some(value), &file_label)?;
            exact_keys(
                license_file,
                &["spdx", "file", "sha256", "registry_source"],
                &file_label,
            )?;
            let spdx = text(license_file.get("spdx"), &format!("{file_label}.spdx"), 1)?;
            if !spdx_identifier(&spdx) {
                return Err(format!("{file_label}.spdx 不是合法 SPDX 标识符"));
            }
            let file = portable_path(license_file.get("file"), &format!("{file_label}.file"))?;
            let sha256 = text(
                license_file.get("sha256"),
                &format!("{file_label}.sha256"),
                1,
            )?;
            if !lower_sha256(&sha256) {
                return Err(format!("{file_label}.sha256 必须是小写 SHA-256"));
            }
            if !seen_spdx.insert(spdx.clone()) || !seen_files.insert(file.to_lowercase()) {
                return Err(format!("{file_label} 重复声明 SPDX 或文件"));
            }
            let registry_source =
                parse_registry_source(license_file.get("registry_source"), &file_label, &spdx)?;
            parsed_files.push(LicenseFile {
                spdx,
                file,
                sha256,
                registry_source,
            });
        }
        if seen_spdx != declared {
            return Err(format!(
                "{label}.license_files 必须逐项覆盖 license_expression，声明={declared:?}，文件={seen_spdx:?}"
            ));
        }
        patches.push(LocalPatch {
            patch: patch_name,
            package,
            version,
            path,
            license_expression,
            license_files: parsed_files,
        });
    }
    Ok(patches)
}

fn parse_registry_source(
    value: Option<&Value>,
    file_label: &str,
    spdx: &str,
) -> PolicyResult<RegistrySource> {
    let label = format!("{file_label}.registry_source");
    let registry = object(value, &label)?;
    let keys = registry.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if keys != BTreeSet::from(["package", "version", "file"])
        && keys != BTreeSet::from(["package", "version", "manifest_license"])
    {
        return Err(format!("{label} 必须选择 file 或 manifest_license 原件"));
    }
    let package = text(registry.get("package"), &format!("{label}.package"), 1)?;
    let version = text(registry.get("version"), &format!("{label}.version"), 1)?;
    if !package_name(&package) {
        return Err(format!("{label}.package 不是合法 Cargo 包名"));
    }
    if !semver(&version) {
        return Err(format!("{label}.version 必须固定到完整三段版本"));
    }
    if registry.contains_key("file") {
        return Ok(RegistrySource::File {
            package,
            version,
            file: portable_path(registry.get("file"), &format!("{label}.file"))?,
        });
    }
    let manifest_license = text(
        registry.get("manifest_license"),
        &format!("{label}.manifest_license"),
        1,
    )?;
    let licenses = license_identifiers(&manifest_license, &format!("{label}.manifest_license"))?;
    if !licenses.iter().any(|license| license == spdx) {
        return Err(format!("{label}.manifest_license 未声明 {spdx}"));
    }
    Ok(RegistrySource::ManifestLicense {
        package,
        version,
        manifest_license,
    })
}

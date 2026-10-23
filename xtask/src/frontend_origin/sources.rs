//! 三端共用 source-inventory.json 的目录规则；覆盖源码、嵌入资产与本地依赖。
use std::{collections::BTreeSet, fs, io, path::Path};

pub const POLICY: &str = "xtask/source-inventory.json";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    format_version: u32,
    files: Vec<String>,
    trees: Vec<String>,
    excluded_directories: Vec<String>,
}

fn policy(root: &Path) -> io::Result<Policy> {
    let bytes = fs::read(root.join(POLICY))?;
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::other("来源规则过大"));
    }
    let value: Policy = serde_json::from_slice(&bytes)?;
    if value.format_version != 1
        || !value.excluded_directories.is_empty()
        || [&value.files, &value.trees]
            .into_iter()
            .flatten()
            .any(|path| !valid_path(path))
    {
        return Err(io::Error::other("来源清单规则无效"));
    }
    Ok(value)
}

fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', ':', '\0', '\r', '\n'])
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn ordinary(path: &Path) -> io::Result<fs::Metadata> {
    let info = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("来源不得包含重解析点"));
        }
    }
    if info.file_type().is_symlink() {
        return Err(io::Error::other("来源不得包含链接"));
    }
    Ok(info)
}

fn walk(root: &Path, relative: &str, output: &mut BTreeSet<String>) -> io::Result<()> {
    let path = root.join(relative);
    let info = ordinary(&path)?;
    if info.is_file() {
        output.insert(relative.to_owned());
    } else if info.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("来源文件名不是 UTF-8"))?;
            walk(root, &format!("{relative}/{name}"), output)?;
        }
    } else {
        return Err(io::Error::other("来源不是普通文件或目录"));
    }
    Ok(())
}

pub fn discover(root: &Path) -> io::Result<Vec<String>> {
    let policy = policy(root)?;
    let mut paths = BTreeSet::new();
    for item in policy.files.into_iter().chain(policy.trees) {
        walk(root, &item, &mut paths)?;
    }
    Ok(paths.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/origin-inventory-tests");
        fs::create_dir_all(&base).unwrap();
        let root = tempfile::tempdir_in(base).unwrap();
        fs::create_dir_all(root.path().join("xtask/src/worker")).unwrap();
        fs::write(root.path().join("xtask/src/worker/control.rs"), "worker").unwrap();
        fs::write(root.path().join(POLICY), r#"{"format_version":1,"files":["xtask/source-inventory.json"],"trees":["xtask/src"],"excluded_directories":[]}"#).unwrap();
        root
    }

    #[test]
    fn complete_sorted_set_tracks_added_removed_and_nested_non_rust_assets() {
        let root = fixture();
        let before = discover(root.path()).unwrap();
        assert_eq!(before.len(), 2);
        let added = root.path().join("xtask/src/worker/规则.sql");
        fs::write(&added, "select 1").unwrap();
        assert_eq!(discover(root.path()).unwrap().len(), 3);
        fs::remove_file(&added).unwrap();
        assert_eq!(discover(root.path()).unwrap(), before);
        fs::remove_file(root.path().join("xtask/src/worker/control.rs")).unwrap();
        assert_eq!(discover(root.path()).unwrap().len(), 1);
    }

    #[test]
    fn rules_reject_duplicate_unknown_fields_traversal_and_invalid_types() {
        let root = fixture();
        let valid = fs::read_to_string(root.path().join(POLICY)).unwrap();
        for raw in [
            valid.replacen('{', "{\"format_version\":1,", 1),
            valid.replace("\"trees\"", "\"unknown\""),
            valid.replace("xtask/src", "../outside"),
            valid.replace("\"format_version\":1", "\"format_version\":true"),
        ] {
            fs::write(root.path().join(POLICY), raw).unwrap();
            assert!(discover(root.path()).is_err());
        }
    }

    #[test]
    fn nested_directory_names_cannot_hide_sources() {
        let root = fixture();
        fs::create_dir(root.path().join("xtask/src/target")).unwrap();
        fs::write(root.path().join("xtask/src/target/cache"), "cache").unwrap();
        assert!(
            discover(root.path())
                .unwrap()
                .contains(&"xtask/src/target/cache".to_owned())
        );
    }

    #[test]
    fn directory_exclusions_are_rejected() {
        let root = fixture();
        let path = root.path().join(POLICY);
        let raw = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            raw.replace(
                "\"excluded_directories\":[]",
                "\"excluded_directories\":[\"target\"]",
            ),
        )
        .unwrap();
        assert!(discover(root.path()).is_err());
    }
}

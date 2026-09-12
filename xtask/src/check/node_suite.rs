use std::{fs, path::Path};

use crate::{Result, process::run_owned};

const NODE_TEST_SUFFIX: &str = ".test.mjs";
const NODE_TEST_CONCURRENCY: &str = "4";

pub(crate) fn discover_node_tests(root: &Path) -> Result<Vec<String>> {
    let test_root = root.join("scripts/tests");
    let mut tests = Vec::new();
    collect_tests(root, &test_root, &mut tests)?;
    tests.sort();
    if tests.is_empty() {
        return Err("scripts/tests 下没有可执行的 *.test.mjs".into());
    }
    Ok(tests)
}

pub(crate) fn run_node_tests(root: &Path) -> Result<()> {
    let tests = discover_node_tests(root)?;
    let mut arguments = vec![
        "--test".to_owned(),
        format!("--test-concurrency={NODE_TEST_CONCURRENCY}"),
    ];
    arguments.extend(tests);
    run_owned(root, "node", &arguments)
}

fn collect_tests(root: &Path, directory: &Path, tests: &mut Vec<String>) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("Node 测试目录必须是真实目录：{}", directory.display()).into());
    }
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if is_link_or_reparse(&metadata) {
            return Err(
                format!("Node 测试目录不接受符号链接或重解析点：{}", path.display()).into(),
            );
        }
        if metadata.is_dir() {
            collect_tests(root, &path, tests)?;
        } else if metadata.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(NODE_TEST_SUFFIX))
        {
            let relative = path.strip_prefix(root)?;
            let relative = relative
                .to_str()
                .ok_or_else(|| format!("Node 测试路径必须是 UTF-8：{}", path.display()))?;
            tests.push(relative.replace('\\', "/"));
        }
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

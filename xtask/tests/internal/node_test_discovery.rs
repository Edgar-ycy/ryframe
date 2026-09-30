use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::check::discover_node_tests;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

#[test]
fn discovery_covers_every_tracked_test_once() {
    let root = backend_root();
    let discovered = discover_node_tests(root).unwrap();
    let output = Command::new("git")
        .args(["ls-files", "--", "tools/js"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut expected = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|path| path.ends_with(".test.mjs"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(discovered, expected);
    assert_eq!(
        discovered.iter().collect::<BTreeSet<_>>().len(),
        discovered.len()
    );
    assert!(discovered.contains(&"tools/js/devex_clone_department_bridge.test.mjs".into()));
    assert!(discovered.contains(&"tools/js/devex_clone_existing.test.mjs".into()));
    assert!(!discovered.iter().any(|path| path.ends_with("_fixture.mjs")));
}

#[test]
fn python_discovery_does_not_duplicate_node_tests() {
    let root = backend_root();
    let output = Command::new("git")
        .args(["ls-files", "--", "tools/python"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());

    for path in String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|path| path.ends_with(".py"))
    {
        let source_path = root.join(path);
        if !source_path.is_file() {
            continue;
        }
        let source = fs::read_to_string(source_path).unwrap();
        assert!(
            !source.contains(".test.mjs"),
            "Python 测试不得重复启动集中发现的 Node 测试：{path}"
        );
    }
}

#[test]
fn discovery_is_recursive_and_deterministic() {
    let root = create_test_directory();
    let nested = root.join("tools/js/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.join("tools/js/z.test.mjs"), "").unwrap();
    fs::write(nested.join("a.test.mjs"), "").unwrap();
    fs::write(nested.join("helper.mjs"), "").unwrap();

    assert_eq!(
        discover_node_tests(&root).unwrap(),
        ["tools/js/nested/a.test.mjs", "tools/js/z.test.mjs"]
    );
    remove_test_directory(&root);
}

fn create_test_directory() -> PathBuf {
    let local_tests = backend_root().join(".local-tests");
    fs::create_dir_all(&local_tests).unwrap();
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = local_tests.join(format!(
        "xtask-node-discovery-{}-{id}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).unwrap();
    root
}

fn remove_test_directory(root: &Path) {
    let test_root = root.join("tools/js");
    let nested = test_root.join("nested");
    fs::remove_file(nested.join("a.test.mjs")).unwrap();
    fs::remove_file(test_root.join("z.test.mjs")).unwrap();
    fs::remove_file(nested.join("helper.mjs")).unwrap();
    fs::remove_dir(nested).unwrap();
    fs::remove_dir(test_root).unwrap();
    fs::remove_dir(root.join("tools")).unwrap();
    fs::remove_dir(root).unwrap();
}

fn backend_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

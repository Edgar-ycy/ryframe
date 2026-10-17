use std::{fs, path::Path};

use super::workspace::root_dir;

fn read(relative: &str) -> String {
    fs::read_to_string(root_dir().join(relative)).unwrap()
}

#[test]
fn release_workflows_use_rust_release_commands_without_python_setup() {
    let release = read(".github/workflows/release.yml");
    let extended = read(".github/workflows/extended-ci.yml");
    for removed in [
        "tools/python/validate_release.py",
        "tools/python/verify_release_ci.py",
        "release_evidence.py",
        "release_private_protocol.py",
    ] {
        assert!(!release.contains(removed), "{removed}");
        assert!(!extended.contains(removed), "{removed}");
    }
    assert!(!release.contains("actions/setup-python@"));
    assert!(!release.contains("安装固定并校验哈希的发布核验 Python 依赖"));
    assert_eq!(
        release.matches("cargo xtask check release source").count(),
        1
    );
    assert_eq!(release.matches("cargo xtask check release ci").count(), 2);
    assert_eq!(
        release.matches("cargo xtask check release ci").count()
            + extended.matches("cargo xtask check release ci").count(),
        4
    );
    assert!(release.contains("--backend-tag-oid"));
    assert!(release.contains("--frontend-tag-oid"));
    assert!(extended.contains("cargo xtask check release ci record-pair"));
    assert!(extended.contains("cargo xtask check release ci verify-pair"));
}

#[test]
fn extended_full_stack_records_before_fixture_and_rechecks_after_build() {
    let extended = read(".github/workflows/extended-ci.yml");
    assert!(extended.contains("fixture: core"));
    assert!(extended.contains("fixture: business"));
    assert!(extended.contains("Linux DevEx Cgroup Memory"));
    let record = extended.find("记录本次全栈源码组合").unwrap();
    let fixture = extended
        .find("在隔离工作树生成业务 crate 并校验幂等性")
        .unwrap();
    let build = extended.find("构建前端生产产物").unwrap();
    let verify = extended.find("复核全栈源码组合未变化").unwrap();
    assert!(record < fixture);
    assert!(fixture < build);
    assert!(build < verify);
}

#[test]
fn current_release_docs_and_help_describe_rust_execution() {
    let operations = read("docs/operations.md");
    assert!(operations.contains("两个入口直接由 Rust xtask 执行，不依赖 Python"));
    assert!(!operations.contains("两个入口都先核验固定 Python 环境"));
    let help = read("xtask/src/cli/help.rs");
    assert!(help.contains("发布核验直接由 Rust xtask 执行"));
    let changelog = read("CHANGELOG.md");
    assert!(changelog.contains("Release 与 Extended CI 不再安装或调用发布核验 Python"));
}

#[test]
fn replaced_release_python_implementations_are_deleted() {
    for path in [
        "tools/python/validate_release.py",
        "tools/python/verify_release_ci.py",
        "tools/python/release_evidence.py",
        "tools/python/release_private_protocol.py",
        "tools/python/test_release_workflow.py",
        "tools/python/test_verify_release_ci.py",
        "tools/python/test_release_evidence.py",
        "tools/python/test_release_private_protocol.py",
    ] {
        assert!(!Path::new(&root_dir()).join(path).exists(), "{path} 仍存在");
    }
}

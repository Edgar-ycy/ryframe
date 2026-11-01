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

#[test]
fn frontend_checkouts_share_repository_and_ref_selection_without_project_environment() {
    for (file, selections) in [
        (
            "ci.yml",
            vec![
                "steps.preflight-frontend-ref.outputs",
                "steps.frontend-ref.outputs",
                "steps.windows-frontend-ref.outputs",
            ],
        ),
        ("extended-ci.yml", vec!["steps.frontend-ref.outputs"]),
    ] {
        let workflow = read(&format!(".github/workflows/{file}"));
        assert!(!workflow.contains("FRONTEND_REPOSITORY"));
        for selection in selections {
            assert!(workflow.contains(&format!("repository: ${{{{ {selection}.repository }}}}")));
            assert!(workflow.contains(&format!("ref: ${{{{ {selection}.ref }}}}")));
        }
    }
    let ci = read(".github/workflows/ci.yml");
    for field in ["repository", "ref"] {
        assert!(ci.contains(&format!("{field}: ${{{{ steps.contract-frontend-ref.outputs.{field} || steps.resource-frontend-ref.outputs.{field} }}}}")));
    }
}

#[test]
fn release_repository_identity_is_selected_before_checkout_and_frozen_for_publication() {
    let release = read(".github/workflows/release.yml");
    assert!(!release.contains("FRONTEND_REPOSITORY"));
    let (validate, publish) = release.split_once("  publish-release:").unwrap();
    assert!(
        validate.find("安装发布核验 Rust 工具链").unwrap()
            < validate
                .find("cargo xtask check ci frontend-source")
                .unwrap()
    );
    assert!(
        validate
            .find("cargo xtask check ci frontend-source")
            .unwrap()
            < validate.find("Check out matching frontend tag").unwrap()
    );
    assert_eq!(
        validate
            .matches("cargo xtask check ci frontend-source")
            .count(),
        1
    );
    let checkout = validate
        .split_once("Check out matching frontend tag")
        .unwrap()
        .1
        .split("      - name:")
        .next()
        .unwrap();
    assert!(checkout.contains("repository: ${{ steps.frontend-source.outputs.repository }}"));
    assert!(checkout.contains("ref: ${{ inputs.tag || github.ref_name }}"));
    assert!(
        validate
            .contains("--frontend-repository \"${{ steps.frontend-source.outputs.repository }}\"")
    );
    assert!(
        validate.contains(
            "echo \"frontend_repository=${{ steps.frontend-source.outputs.repository }}\""
        )
    );
    assert!(
        validate.contains("frontend_repository: ${{ steps.release.outputs.frontend_repository }}")
    );
    assert!(
        validate
            .contains("--frontend-repository \"${{ steps.release.outputs.frontend_repository }}\"")
    );
    assert!(
        publish.contains("repository: ${{ needs.validate-release.outputs.frontend_repository }}")
    );
    assert!(publish.contains(
        "--frontend-repository \"${{ needs.validate-release.outputs.frontend_repository }}\""
    ));
    assert!(!publish.contains("frontend-source"));
    assert!(!release.contains("steps.frontend-source.outputs.ref"));
}

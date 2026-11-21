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
    use super::ci_workflow_tests::{argument, command, job, text, workflow};
    let extended = workflow("extended-ci.yml");
    assert!(extended.get("jobs").unwrap().get("devex-cgroup").is_some());
    let full_stack = job(&extended, "full-stack-e2e");
    let matrix = full_stack
        .get("strategy")
        .unwrap()
        .get("matrix")
        .unwrap()
        .get("include")
        .unwrap()
        .as_sequence()
        .unwrap();
    for required in ["core", "business"] {
        assert!(matrix.iter().any(|row| text(row, "fixture") == required));
    }
    let (record, record_step) = command(full_stack, "cargo xtask check release ci record-pair");
    let (fixture, fixture_step) = command(full_stack, "cargo xtask check recovery fixture");
    let (config, config_step) = command(full_stack, "cargo xtask check ci full-stack-config");
    let tool = r#""$RUNNER_TEMP/ryframe-full-stack-tool/debug/xtask" check recovery full-stack"#;
    let (start, _) = command(full_stack, &format!("{tool} start"));
    let (core_build, core_step) = command(full_stack, "corepack pnpm build");
    let frontend_command = format!("{tool} frontend");
    let (business_build, business_step) = command(full_stack, &frontend_command);
    let (verify, verify_step) = command(full_stack, "cargo xtask check release ci verify-pair");
    assert!(
        record < fixture
            && fixture < config
            && config < start
            && start < core_build
            && core_build < business_build
            && business_build < verify
    );
    for (step, directory) in [
        (record_step, "backend"),
        (fixture_step, "backend"),
        (config_step, "${{ matrix.backend }}"),
        (core_step, "${{ matrix.frontend }}"),
        (business_step, "${{ matrix.backend }}"),
        (verify_step, "backend"),
    ] {
        assert_eq!(text(step, "working-directory"), directory);
    }
    assert_eq!(
        argument(record_step, "--output"),
        "$RUNNER_TEMP/ryframe-full-stack/source-pair.json"
    );
    assert_eq!(
        argument(verify_step, "--input"),
        argument(record_step, "--output")
    );
    for step in [record_step, fixture_step, verify_step] {
        assert_eq!(
            argument(step, "--frontend-dir"),
            "$GITHUB_WORKSPACE/frontend"
        );
    }
    assert_eq!(
        text(fixture_step, "if"),
        "${{ matrix.fixture == 'business' }}"
    );
    assert_eq!(
        argument(fixture_step, "--output-dir"),
        "$GITHUB_WORKSPACE/backend/.local-tests/business-fixture"
    );
    assert_eq!(
        argument(core_step, "--source-commit"),
        "$(git rev-parse HEAD)"
    );
    assert_eq!(text(core_step, "if"), "${{ matrix.fixture == 'core' }}");
    assert_eq!(
        text(business_step, "if"),
        "${{ matrix.fixture == 'business' }}"
    );
    assert_eq!(text(business_step, "run"), frontend_command);
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

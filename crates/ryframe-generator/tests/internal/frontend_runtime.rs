use super::*;

fn fixture() -> tempfile::TempDir {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/frontend-runtime");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

fn package(root: &Path, source: &[u8], installed: bool) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("package.json"), source).unwrap();
    if installed {
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(
            root.join("node_modules/.modules.yaml"),
            "layoutVersion: 5\n",
        )
        .unwrap();
    }
}

#[test]
fn source_only_candidate_reuses_matching_installed_ancestor() {
    let fixture = fixture();
    let original = fixture.path().join("前端 with spaces");
    let candidate = original.join(".local-tests/candidate/workspace");
    package(&original, b"{\"name\":\"frontend\"}", true);
    package(&candidate, b"{\"name\":\"frontend\"}", false);
    assert_eq!(runtime_root(&candidate).unwrap(), original);
    assert!(!candidate.join("node_modules").exists());
}

#[test]
fn nearest_installed_environment_mismatch_does_not_fall_back() {
    let fixture = fixture();
    let original = fixture.path().join("frontend");
    let middle = original.join(".local-tests/middle");
    let candidate = middle.join("workspace");
    package(&original, b"{\"name\":\"wanted\"}", true);
    package(&middle, b"{\"name\":\"different\"}", true);
    package(&candidate, b"{\"name\":\"wanted\"}", false);
    let error = runtime_root(&candidate).unwrap_err();
    assert!(error.message.contains("不一致"));
    assert!(error.message.contains("本批未安装"));
}

#[test]
fn missing_installed_environment_is_rejected_without_installing() {
    let fixture = fixture();
    let frontend = fixture.path().join("frontend");
    package(&frontend, b"{\"name\":\"frontend\"}", false);
    let error = runtime_root(&frontend).unwrap_err();
    assert!(error.message.contains("没有找到匹配"));
    assert!(
        error
            .suggestion
            .contains("corepack pnpm install --frozen-lockfile")
    );
    assert_eq!(fs::read_dir(frontend).unwrap().count(), 1);
}

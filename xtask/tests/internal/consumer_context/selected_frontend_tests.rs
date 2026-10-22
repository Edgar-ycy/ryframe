use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{absolute_directory, selected_frontend_directory, tests::link_directory};

fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/consumer-selection-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn frontend_configuration(backend: &Path, relative: &str) -> PathBuf {
    fs::create_dir_all(backend.join("config")).unwrap();
    let value = serde_json::json!({"workspace": {"frontend_dir":relative}});
    fs::write(
        backend.join("config/tools.toml"),
        toml::to_string(&value).unwrap(),
    )
    .unwrap();
    ryframe_config::ToolSettings::load(backend)
        .unwrap()
        .frontend_dir(backend)
}

#[test]
fn default_sibling_configuration_is_selected_before_contract_plan_loading() {
    let root = fixture();
    let backend = root.path().join("backend");
    let frontend = root.path().join("ryframe-vue3");
    fs::create_dir_all(frontend.join("openapi")).unwrap();
    fs::write(
        frontend.join("openapi/source.json"),
        serde_json::to_vec(&serde_json::json!({
            "backend_repository":"owner/repo", "backend_commit":"1".repeat(40)
        }))
        .unwrap(),
    )
    .unwrap();
    let configured = frontend_configuration(&backend, "../ryframe-vue3");
    assert!(
        configured
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    );
    let selected = selected_frontend_directory(&backend, &configured).unwrap();
    assert_eq!(selected, absolute_directory(&frontend).unwrap());
    assert_eq!(
        selected_frontend_directory(&backend, Path::new("../ryframe-vue3")).unwrap(),
        selected
    );
    assert_eq!(
        selected_frontend_directory(&backend, Path::new("./../ryframe-vue3/.")).unwrap(),
        selected
    );
    let plan = crate::check::snapshot::load_consumer_contract_plan(&selected, None).unwrap();
    assert_eq!(plan.mode, "formal");
    assert_eq!(plan.backend_commit, "1".repeat(40));
}

#[test]
fn renamed_sibling_with_spaces_and_chinese_uses_the_updated_configuration() {
    let root = fixture();
    let backend = root.path().join("backend");
    let frontend = root.path().join("ryframe-vue3");
    fs::create_dir(&frontend).unwrap();
    let before = frontend_configuration(&backend, "../ryframe-vue3");
    assert!(selected_frontend_directory(&backend, &before).is_ok());
    let renamed = root.path().join("前端 新名字 web");
    fs::rename(&frontend, &renamed).unwrap();
    let configured = frontend_configuration(&backend, "../前端 新名字 web");
    assert_eq!(
        selected_frontend_directory(&backend, &configured).unwrap(),
        absolute_directory(&renamed).unwrap()
    );
    assert!(selected_frontend_directory(&backend, &before).is_err());
}

#[test]
fn link_followed_by_parent_traversal_is_rejected_before_any_normalization() {
    let root = fixture();
    let backend = root.path().join("backend");
    fs::create_dir_all(backend.join("ryframe-vue3")).unwrap();
    fs::create_dir(root.path().join("ryframe-vue3")).unwrap();
    let target = root.path().join("link-target");
    fs::create_dir(&target).unwrap();
    let link = backend.join("alias");
    link_directory(&link, &target);
    let original = link.join("../ryframe-vue3");
    assert!(selected_frontend_directory(&backend, &original).is_err());
    // 两种先行规范化都会隐藏真实前缀，不能据此接受用户选择。
    assert!(fs::canonicalize(&original).is_ok());
    assert!(backend.join("ryframe-vue3").is_dir());
}

#[test]
fn missing_or_regular_file_prefix_cannot_be_erased_by_parent_traversal() {
    let root = fixture();
    let backend = root.path().join("backend");
    fs::create_dir_all(backend.join("ryframe-vue3")).unwrap();
    let missing = backend.join("missing/../ryframe-vue3");
    assert!(selected_frontend_directory(&backend, &missing).is_err());
    assert!(!backend.join("missing").exists());
    fs::write(backend.join("regular-file"), b"not a directory").unwrap();
    let regular = backend.join("regular-file/../ryframe-vue3");
    assert!(selected_frontend_directory(&backend, &regular).is_err());
}

#[test]
fn parent_traversal_cannot_cross_the_filesystem_root() {
    let root = fixture();
    let base = absolute_directory(root.path()).unwrap();
    let filesystem_root = base.ancestors().last().unwrap();
    assert!(selected_frontend_directory(&base, &filesystem_root.join("..")).is_err());
}

#[test]
fn external_context_and_source_paths_still_reject_original_parent_traversal() {
    let root = fixture();
    let backend = root.path().join("backend");
    let frontend = root.path().join("ryframe-vue3");
    fs::create_dir(&frontend).unwrap();
    let original = frontend_configuration(&backend, "../ryframe-vue3");
    let selected = selected_frontend_directory(&backend, &original).unwrap();
    assert!(
        !selected
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    );
    assert!(super::raw_path(&original).is_err());
    assert!(absolute_directory(&original).is_err());
    assert!(crate::check::consumer_context::prepare(&backend, &original, Vec::new()).is_err());
    assert!(!backend.join(".local-tests/consumer-contexts").exists());
}

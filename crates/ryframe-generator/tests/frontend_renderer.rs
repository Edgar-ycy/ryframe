use std::{
    fs,
    path::{Path, PathBuf},
};

use ryframe_generator::{load_resource, render_frontend_assets};

#[test]
fn frontend_renderer_returns_exact_framework_assets_without_writing() {
    let Some(frontend) = std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR") else {
        eprintln!("跨仓渲染字节验证未执行：需显式 RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR");
        return;
    };
    let frontend = PathBuf::from(frontend);
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let resources = ["notice", "post"]
        .map(|name| load_resource(backend.join(format!("catalog/resources/{name}.toml"))).unwrap());
    let output = render_frontend_assets(&resources, &frontend, true).unwrap();
    assert!(!output.is_empty());
    for asset in output {
        assert_eq!(
            asset.content.as_bytes(),
            fs::read(frontend.join(&asset.path)).unwrap(),
            "{}",
            asset.path,
        );
    }
    assert!(
        render_frontend_assets(&[], &frontend, false)
            .unwrap()
            .is_empty()
    );
    let empty = render_frontend_assets(&[], &frontend, true).unwrap();
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].resource, "__catalog__");
}

#[test]
fn missing_frontend_fails_before_resource_installation() {
    let parent =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/frontend-renderer");
    fs::create_dir_all(&parent).unwrap();
    let frontend = tempfile::tempdir_in(parent).unwrap();
    let error = render_frontend_assets(&[], frontend.path(), false).unwrap_err();
    assert!(error.message.contains("入口"));
    assert!(error.message.contains("本批未安装"));
    assert!(error.suggestion.contains("workspace.frontend_dir"));
    assert_eq!(fs::read_dir(frontend.path()).unwrap().count(), 0);
}

#[test]
fn frontend_renderer_works_in_source_only_candidate_with_chinese_and_spaces() {
    let Some(frontend) = std::env::var_os("RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR") else {
        eprintln!("候选路径渲染验证未执行：需显式 RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR");
        return;
    };
    let frontend = PathBuf::from(frontend);
    let parent = frontend.join(".local-tests/frontend-renderer-candidate");
    fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::tempdir_in(parent).unwrap();
    let candidate = temporary.path().join("候选 front end");
    fs::create_dir(&candidate).unwrap();
    fs::copy(
        frontend.join("package.json"),
        candidate.join("package.json"),
    )
    .unwrap();
    copy_renderer(
        &frontend.join("scripts/resource-render"),
        &candidate.join("scripts/resource-render"),
    );
    let post = load_resource(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources/post.toml"),
    )
    .unwrap();
    let result = render_frontend_assets(&[post], &candidate, false);
    let assets = match result {
        Ok(assets) => assets,
        Err(error) => panic!(
            "候选渲染失败，现场保留于 {}：{error}",
            temporary.keep().display()
        ),
    };
    assert!(assets.len() >= 5);
    for asset in assets {
        assert_eq!(
            asset.content.as_bytes(),
            fs::read(frontend.join(asset.path)).unwrap()
        );
    }
    assert!(!candidate.join("src").exists());
    assert!(!candidate.join(".ryframe").exists());
    assert!(!candidate.join("node_modules").exists());
    assert!(!candidate.join("pnpm-lock.yaml").exists());
}

fn copy_renderer(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_renderer(&entry.path(), &destination);
        } else {
            assert!(kind.is_file());
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

use std::{
    fs,
    path::{Path, PathBuf},
};

use ryframe_generator::{
    AssetRoot, GeneratedAsset, PlanAction, ResourceWorkspace, load_resource, plan_resource_changes,
    render_frontend_assets, render_resources_with_frontend, write_resource, write_resources,
};

#[path = "support/resource_workspace.rs"]
mod resource_workspace_control;

#[test]
fn frontend_renderer_returns_exact_framework_assets_without_writing() {
    let Ok((frontend, _, _)) = resource_workspace_control::load() else {
        eprintln!("跨仓渲染字节验证未执行：需从受管控制目录启动");
        return;
    };
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
    let Ok((frontend, _, _)) = resource_workspace_control::load() else {
        eprintln!("候选路径渲染验证未执行：需从受管控制目录启动");
        return;
    };
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

#[test]
fn explicit_frontend_writes_preserve_selection_and_delete_only_obsolete_assets() {
    let Ok((frontend, _, _)) = resource_workspace_control::load() else {
        eprintln!("跨仓受管写入验证未执行：需从受管控制目录启动");
        return;
    };
    let backend_parent =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/frontend-writer");
    let frontend_parent = frontend.join(".local-tests/frontend-writer");
    fs::create_dir_all(&backend_parent).unwrap();
    fs::create_dir_all(&frontend_parent).unwrap();
    let backend = tempfile::tempdir_in(backend_parent).unwrap();
    let target = tempfile::tempdir_in(frontend_parent).unwrap();
    let result = std::panic::catch_unwind(|| {
        verify_selected_frontend_writes(&frontend, backend.path(), target.path())
    });
    if let Err(error) = result {
        eprintln!(
            "跨仓失败源码保留：后端 {}，前端 {}",
            backend.keep().display(),
            target.keep().display()
        );
        std::panic::resume_unwind(error);
    }
}

fn verify_selected_frontend_writes(source: &Path, backend: &Path, frontend: &Path) {
    fs::write(
        backend.join("Cargo.toml"),
        "[workspace.package]\nversion='0.13.1'\n",
    )
    .unwrap();
    let catalog_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources");
    let resources = ["notice", "post"]
        .map(|name| load_resource(catalog_root.join(format!("{name}.toml"))).unwrap());
    let catalog = render_resources_with_frontend(&resources, source).unwrap();
    let workspace = ResourceWorkspace {
        backend_root: backend,
        frontend_root: Some(frontend),
    };
    write_resource(&catalog, "post", workspace).unwrap();
    let post = frontend.join("src/generated/resources/post/api.ts");
    let post_before = fs::read(&post).unwrap();
    let aggregate = frontend.join("src/generated/resources/index.ts");
    let first_aggregate = fs::read_to_string(&aggregate).unwrap();
    assert!(first_aggregate.contains("post"));
    assert!(!first_aggregate.contains("notice"));
    assert!(!frontend.join("src/generated/resources/notice").exists());
    write_resource(&catalog, "notice", workspace).unwrap();
    assert_eq!(fs::read(&post).unwrap(), post_before);
    let second_aggregate = fs::read_to_string(&aggregate).unwrap();
    assert!(second_aggregate.contains("post") && second_aggregate.contains("notice"));
    assert!(
        write_resource(&catalog, "notice", workspace)
            .unwrap()
            .written
            .is_empty()
    );

    let obsolete = "src/generated/resources/post/obsolete.ts";
    let mut previous = catalog.clone();
    previous.assets.push(GeneratedAsset {
        resource: "post".into(),
        root: AssetRoot::Frontend,
        path: obsolete.into(),
        content: "// 已废弃的受管测试资产\n".into(),
    });
    write_resources(&previous, workspace).unwrap();
    let notice = frontend.join("src/generated/resources/notice/api.ts");
    let notice_before = fs::read(&notice).unwrap();
    let plan = plan_resource_changes(&catalog, "post", workspace).unwrap();
    let deletion = plan
        .assets
        .iter()
        .find(|asset| asset.path == obsolete)
        .unwrap();
    assert_eq!(deletion.root, AssetRoot::Frontend);
    assert_eq!(deletion.action, PlanAction::Delete);
    assert!(!deletion.before.is_empty() && deletion.after.is_empty());
    assert!(frontend.join(obsolete).is_file());
    let written = write_resource(&catalog, "post", workspace).unwrap();
    assert!(written.removed.iter().any(|path| path.ends_with(obsolete)));
    assert!(!frontend.join(obsolete).exists());
    assert_eq!(fs::read(&notice).unwrap(), notice_before);
    assert_eq!(fs::read(&post).unwrap(), post_before);

    fs::write(
        backend.join("crates/ryframe-application/src/generated/post/model.rs"),
        "// 人工改动\n",
    )
    .unwrap();
    assert!(
        write_resources(&catalog, workspace)
            .unwrap_err()
            .message
            .contains("人工修改")
    );
    assert_eq!(fs::read(&notice).unwrap(), notice_before);
    assert_eq!(fs::read(&post).unwrap(), post_before);
}

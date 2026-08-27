use std::{fs, path::PathBuf};

use ryframe_generator::{
    PlanAction, ResourceWorkspace, load_resource, plan_all_resource_changes, plan_resource_changes,
    render_resources, write_resources,
};

fn device_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device.toml")
}

#[test]
fn all_resource_check_plan_is_read_only_and_reaches_zero_diff() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(backend.path().join("Cargo.toml"), "[workspace]\n").expect("应创建工作区标识");
    let device = load_resource(device_path()).expect("Device 资源清单应有效");
    let catalog = render_resources(&[device]).expect("生成应成功");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    write_resources(&catalog, workspace).expect("首次写入应成功");

    let clean = plan_all_resource_changes(&catalog, workspace).expect("全量检查应成功");
    assert!(
        clean
            .assets
            .iter()
            .all(|asset| asset.action == PlanAction::Unchanged),
        "写入后全量检查必须为零差异"
    );

    let model_path = backend
        .path()
        .join("crates/ryframe-application/src/generated/device/model.rs");
    let ownership_path = backend.path().join("catalog/resources/.ownership.toml");
    let model_before = fs::read(&model_path).expect("模型文件应存在");
    let ownership_before = fs::read(&ownership_path).expect("ownership 应存在");
    let mut changed = catalog.clone();
    changed
        .assets
        .iter_mut()
        .find(|asset| asset.path.ends_with("generated/device/model.rs"))
        .expect("应存在 Device 模型")
        .content
        .push_str("// 生成器新版本输出\n");

    let plan = plan_all_resource_changes(&changed, workspace).expect("全量检查应报告差异");
    assert!(plan.assets.iter().any(|asset| {
        asset.path.ends_with("generated/device/model.rs") && asset.action == PlanAction::Update
    }));
    assert!(plan.assets.iter().any(|asset| {
        asset.path == "catalog/resources/.ownership.toml" && asset.action == PlanAction::Update
    }));
    assert_eq!(fs::read(&model_path).unwrap(), model_before);
    assert_eq!(fs::read(&ownership_path).unwrap(), ownership_before);

    write_resources(&changed, workspace).expect("显式写入应应用计划");
    let clean = plan_all_resource_changes(&changed, workspace).expect("写入后应重新检查");
    assert!(
        clean
            .assets
            .iter()
            .all(|asset| asset.action == PlanAction::Unchanged)
    );

    let stale_manifest = fs::read_to_string(&ownership_path)
        .expect("ownership 应存在")
        .replace(
            ryframe_generator::GENERATOR_VERSION,
            "stale-generator-version",
        );
    fs::write(&ownership_path, &stale_manifest).expect("应模拟旧 generator version");
    let named = plan_resource_changes(&changed, "device", workspace).expect("命名检查应成功");
    assert!(named.assets.iter().any(|asset| {
        asset.path == "catalog/resources/.ownership.toml" && asset.action == PlanAction::Update
    }));
    assert_eq!(fs::read_to_string(&ownership_path).unwrap(), stale_manifest);
}

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ryframe_generator::{
    AssetRoot, OwnershipManifest, RelationIr, RelationKind, ResourceWorkspace, load_resource,
    render_resources_with_frontend, write_resource,
};

#[path = "resource_contracts/business_compilation.rs"]
mod business_compilation;
#[path = "resource_contracts/frontend_workspace.rs"]
mod frontend_workspace;
#[path = "generation_pipeline/fixture.rs"]
mod generation_pipeline;
#[path = "support/resource_workspace.rs"]
mod resource_workspace_control;
#[path = "../src/command/staging/snapshot.rs"]
mod snapshot;
#[path = "resource_contracts/unsigned_compilation.rs"]
mod unsigned_compilation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationProfile {
    Full,
    Targeted,
}

impl VerificationProfile {
    fn from_value(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(Self::Full),
            Some("targeted") => Ok(Self::Targeted),
            Some(value) => Err(format!(
                "资源 Workspace 验证模式 `{value}` 无效；仅 targeted gate 可设置 targeted"
            )),
        }
    }

    fn from_control(value: &str) -> Result<Self, String> {
        Self::from_value((value != "full").then_some(value))
    }
}

struct SharedWorkspace {
    backend_source: PathBuf,
    frontend_source: PathBuf,
    backend: PathBuf,
    frontend: PathBuf,
    cargo_target: PathBuf,
}

#[test]
#[ignore = "完整门禁在临时真实 Workspace 中验证标准资源、业务 crate、Cargo 与 vue-tsc"]
fn generated_slices_and_business_crates_compile_in_real_workspaces() {
    run_shared_workspace().expect("标准资源与业务 crate 的完整 Workspace 验证失败");
}

#[test]
fn verification_profile_defaults_to_full_and_rejects_unknown_values() {
    assert_eq!(
        VerificationProfile::from_value(None).unwrap(),
        VerificationProfile::Full
    );
    assert_eq!(
        VerificationProfile::from_value(Some("targeted")).unwrap(),
        VerificationProfile::Targeted
    );
    for value in ["", "full", "targeted ", "unknown"] {
        assert!(
            VerificationProfile::from_value(Some(value)).is_err(),
            "未知模式 {value:?} 必须失败关闭"
        );
    }
}

#[test]
#[ignore = "需要显式前端目录与已安装依赖，验证实际标准输入协议"]
fn order_frontend_fixture_protocol_is_repeatable_and_preserves_source() {
    let (source, _, _) =
        resource_workspace_control::load().expect("应提供受管资源 Workspace 控制文件");
    let parent = source.join(".local-tests/resource-workspace-contract");
    fs::create_dir_all(&parent).unwrap();
    let scope = tempfile::tempdir_in(parent).unwrap();
    let files = [
        "src/api/generated/permissions.ts",
        "src/api/generated/operations/system.ts",
        "src/api/generated/schema/system.ts",
    ];
    let before = files.map(|file| fs::read(source.join(file)).unwrap());
    for (file, bytes) in files.iter().zip(&before) {
        let target = scope.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let result = std::panic::catch_unwind(|| {
        frontend_workspace::register(&source, scope.path());
        let first = files.map(|file| fs::read(scope.path().join(file)).unwrap());
        frontend_workspace::register(&source, scope.path());
        let repeated = files.map(|file| fs::read(scope.path().join(file)).unwrap());
        assert_eq!(first, repeated);
        assert_eq!(
            files.map(|file| fs::read(source.join(file)).unwrap()),
            before
        );
    });
    if let Err(error) = result {
        eprintln!("前端契约协议失败现场保留：{}", scope.keep().display());
        std::panic::resume_unwind(error);
    }
}

fn run_shared_workspace() -> Result<(), String> {
    let (frontend_source, profile, target_dir) = resource_workspace_control::load()?;
    let profile = VerificationProfile::from_control(&profile)?;
    let workspace = prepare_shared_workspace(profile, frontend_source, target_dir)?;
    generate_resource_slices(&workspace, profile);
    if profile == VerificationProfile::Targeted {
        return Ok(());
    }
    register_order_capability(&workspace.backend);
    register_generated_backend_modules(&workspace.backend);
    frontend_workspace::register(&workspace.frontend_source, &workspace.frontend);
    write_order_fake_transaction_test(&workspace.backend);
    unsigned_compilation::prepare(&workspace.backend);
    assert_backend_checks(&workspace);
    business_compilation::verify(&workspace.backend, &workspace.cargo_target);
    assert_frontend_checks(&workspace.frontend_source, &workspace.frontend);
    generation_pipeline::verify(&workspace.frontend_source, &workspace.cargo_target);
    Ok(())
}

fn register_order_capability(backend: &Path) {
    let catalog = backend.join("catalog/access.toml");
    let mut source = fs::read_to_string(&catalog).expect("应读取临时访问目录");
    source.push_str(
        r#"

[[capabilities]]
code = "business.order"
route_keys = ["business.order"]
page_keys = ["business.order"]
permissions = ["system:order:create", "system:order:read", "system:order:list", "system:order:update", "system:order:delete"]
"#,
    );
    fs::write(catalog, source).expect("应在临时副本登记设备访问能力");

    let application =
        backend.join("crates/ryframe-application/src/system/product_capability_catalog.rs");
    let source = fs::read_to_string(&application).expect("应读取临时产品能力目录");
    let marker = "];\n\npub fn capability_descriptor";
    let capability = r#"    standard_capability(
        "business.order",
        "设备管理",
        &["business.order"],
        &[
            "system:order:create",
            "system:order:read",
            "system:order:list",
            "system:order:update",
            "system:order:delete",
        ],
        &[],
    ),
];

pub fn capability_descriptor"#;
    let source = source.replacen(marker, capability, 1);
    assert_ne!(
        source,
        fs::read_to_string(&application).unwrap(),
        "产品能力目录标记必须存在"
    );
    fs::write(application, source).expect("应在临时副本登记设备产品能力");
}

fn prepare_shared_workspace(
    profile: VerificationProfile,
    frontend_source: PathBuf,
    cargo_target: PathBuf,
) -> Result<SharedWorkspace, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("无法读取资源 Workspace 当前目录：{error}"))?;
    let backend_source = current_dir
        .ancestors()
        .find(|path| {
            path.join("catalog/resources").is_dir()
                && path
                    .join("crates/ryframe-generator/tests/fixtures/order.toml")
                    .is_file()
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("无法从 {} 定位后端 Workspace 根目录", current_dir.display()))?;
    if !frontend_source.join("node_modules").is_dir() {
        return Err("前端 node_modules 不存在".into());
    }

    let backend_parent = backend_source.join(".local-tests");
    let frontend_parent = frontend_source.join(".local-tests");
    fs::create_dir_all(&backend_parent).expect("应创建后端临时测试目录");
    fs::create_dir_all(&frontend_parent).expect("应创建前端临时测试目录");
    let backend = backend_parent.join("shared-resource-workspace");
    let frontend = frontend_parent.join("shared-resource-frontend");
    reset_workspace_directory(&backend);
    reset_workspace_directory(&frontend);

    if profile == VerificationProfile::Full {
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "rustfmt.toml",
        ] {
            sync_file(&backend_source.join(file), &backend.join(file));
        }
        for directory in [".cargo", "catalog", "crates", "vendor", "xtask"] {
            sync_directory(&backend_source.join(directory), &backend.join(directory));
        }
        prepare_frontend_workspace(&frontend_source, &frontend);
    } else {
        // targeted 只验证生成计划和重复写入，不运行临时 Workspace 编译；保留生成器
        // 用于识别根目录的两个标记文件即可，避免复制 crates/vendor 与完整前端源码。
        sync_file(
            &backend_source.join("Cargo.toml"),
            &backend.join("Cargo.toml"),
        );
        sync_file(
            &frontend_source.join("package.json"),
            &frontend.join("package.json"),
        );
    }

    Ok(SharedWorkspace {
        backend_source,
        frontend_source,
        backend,
        frontend,
        cargo_target,
    })
}

fn reset_workspace_directory(path: &Path) {
    if path.exists() {
        fs::remove_dir_all(path)
            .unwrap_or_else(|error| panic!("清理临时 Workspace {} 失败：{error}", path.display()));
    }
    fs::create_dir_all(path)
        .unwrap_or_else(|error| panic!("创建临时 Workspace {} 失败：{error}", path.display()));
}

fn generate_resource_slices(workspace: &SharedWorkspace, profile: VerificationProfile) {
    let mut order = load_auto_order();
    order.relations.push(RelationIr {
        name: "parent".into(),
        pascal_name: "Parent".into(),
        kind: RelationKind::BelongsTo,
        local_field: "id".into(),
        target_resource: "order".into(),
        target_pascal_name: "Order".into(),
    });
    let post = load_resource(workspace.backend_source.join("catalog/resources/post.toml"))
        .expect("临时 Workspace 中既有的 Post 清单应有效");
    let notice = load_resource(
        workspace
            .backend_source
            .join("catalog/resources/notice.toml"),
    )
    .expect("临时 Workspace 中既有的 Notice 清单应有效");
    let catalog =
        render_resources_with_frontend(&[order, notice, post], &workspace.frontend_source)
            .expect("Order 与既有资源应能共同生成");
    let initial_resources: &[&str] = if profile == VerificationProfile::Targeted {
        &["notice", "post", "order"]
    } else {
        &["order"]
    };
    for resource in initial_resources {
        let first = write_resource(
            &catalog,
            resource,
            ResourceWorkspace {
                backend_root: &workspace.backend,
                frontend_root: Some(&workspace.frontend),
            },
        )
        .unwrap_or_else(|error| panic!("{resource} 首次生成应成功：{error}"));
        assert!(!first.written.is_empty(), "{resource} 首次生成必须写入资产");
    }
    assert_generated_ownership(workspace);
    for path in [
        "crates/ryframe-application/src/generated/post/service.rs",
        "crates/ryframe-application/src/generated/notice/service.rs",
        "crates/ryframe-api/src/generated/notice/handler.rs",
        "crates/ryframe-db/src/generated/post/repository.rs",
        "crates/ryframe-db/src/generated/notice/repository.rs",
        "src/generated/resources/post/page.vue",
        "src/generated/resources/notice/page.vue",
        "src/generated/resources/notice/registration.ts",
    ] {
        let root = if path.starts_with("src/") {
            &workspace.frontend
        } else {
            &workspace.backend
        };
        assert!(root.join(path).is_file(), "Post 资产未生成：{path}");
    }
    for resource in ["order", "notice", "post"] {
        let repeated = write_resource(
            &catalog,
            resource,
            ResourceWorkspace {
                backend_root: &workspace.backend,
                frontend_root: Some(&workspace.frontend),
            },
        )
        .unwrap_or_else(|error| panic!("{resource} 连续生成应成功：{error}"));
        assert!(
            repeated.written.is_empty(),
            "{resource} 连续生成不得写入：{:?}",
            repeated.written
        );
        assert!(
            repeated.removed.is_empty(),
            "{resource} 连续生成不得删除：{:?}",
            repeated.removed
        );
    }
    let notice_registration = fs::read_to_string(
        workspace
            .frontend
            .join("src/generated/resources/notice/registration.ts"),
    )
    .expect("应读取 Notice 页面注册清单");
    assert!(
        notice_registration.contains("@/views/system/notice/index.vue"),
        "Notice 必须保留强类型自定义页面扩展"
    );
}

fn assert_generated_ownership(workspace: &SharedWorkspace) {
    let manifest_path = workspace.backend.join("catalog/resources/.ownership.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("应读取生成后的 ownership manifest");
    let manifest: OwnershipManifest = toml::from_str(&manifest).expect("ownership manifest 应有效");
    let selected = ["order", "notice", "post"];
    let mut roots = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| selected.contains(&entry.resource.as_str()))
    {
        assert!(
            paths.insert((entry.root, entry.path.as_str())),
            "ownership 不得重复登记路径：{}",
            entry.path
        );
        let root = match entry.root {
            AssetRoot::Backend => &workspace.backend,
            AssetRoot::Frontend => &workspace.frontend,
        };
        assert!(
            root.join(&entry.path).is_file(),
            "ownership 资产不存在：{}",
            entry.path
        );
        roots.insert((entry.resource.as_str(), entry.root));
    }
    for resource in selected {
        for root in [AssetRoot::Backend, AssetRoot::Frontend] {
            assert!(
                roots.contains(&(resource, root)),
                "{resource} 缺少 {} ownership",
                root.label()
            );
        }
    }
}

fn register_generated_backend_modules(backend: &Path) {
    for crate_name in [
        "ryframe-application",
        "ryframe-db",
        "ryframe-tenant-db",
        "ryframe-api",
    ] {
        let lib = backend.join("crates").join(crate_name).join("src/lib.rs");
        let mut source = fs::read_to_string(&lib).expect("应读取 crate lib.rs");
        if !source
            .lines()
            .any(|line| line.trim() == "pub mod generated;")
        {
            source.push_str("\npub mod generated;\n");
        }
        fs::write(&lib, source).expect("应在临时副本接入 generated module");
    }
}

fn load_auto_order() -> ryframe_generator::ResourceIr {
    // 同时验证租户复合主键自增与 Post/Notice 默认雪花主键的真实编译面。
    let mut spec =
        ryframe_generator::ResourceSpec::parse(include_str!("fixtures/order.toml"), "order.toml")
            .unwrap();
    spec.database.id_generation = ryframe_generator::IdGeneration::AutoIncrement;
    spec.fields
        .iter_mut()
        .find(|field| field.name == "name")
        .unwrap()
        .sql_comment = Some("注释 '原样'\\路径".into());
    spec.database.indexes.push(ryframe_generator::IndexSpec {
        name: "idx_order_auto_id".into(),
        fields: vec!["id".into()],
        unique: false,
    });
    ryframe_generator::normalize_resource(spec, "order.toml", "auto-order-fixture").unwrap()
}

fn assert_backend_checks(workspace: &SharedWorkspace) {
    let cargo_fmt = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(&workspace.backend)
        .output()
        .expect("应检查生成 Rust 资产格式");
    assert_command_succeeded("cargo fmt --check", &cargo_fmt);

    let migration = Command::new("cargo")
        .args([
            "check",
            "-p",
            "ryframe-db",
            "-p",
            "ryframe-tenant-db",
            "--no-default-features",
            "--features",
            "ryframe-db/migration,ryframe-tenant-db/migration",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行生成迁移最小面 cargo check");
    assert_command_succeeded("cargo check generated migrations", &migration);

    let cargo = Command::new("cargo")
        .args([
            "check",
            "-p",
            "ryframe-application",
            "-p",
            "ryframe-db",
            "-p",
            "ryframe-tenant-db",
            "-p",
            "ryframe-api",
            "--no-default-features",
            "--features",
            "ryframe-db/repositories,ryframe-tenant-db/repositories",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行临时后端 cargo check");
    assert_command_succeeded("cargo check generated repositories", &cargo);

    let fake_test = Command::new("cargo")
        .args([
            "test",
            "-p",
            "ryframe-application",
            "--no-default-features",
            "--features",
            "test-support",
            "--test",
            "generated_order_fake",
            "--test",
            "generated_unsigned_fake",
        ])
        .current_dir(&workspace.backend)
        .env("CARGO_TARGET_DIR", &workspace.cargo_target)
        .output()
        .expect("应运行生成 Fake 事务语义测试");
    assert_command_succeeded("generated Order fake transaction test", &fake_test);
}

fn prepare_frontend_workspace(source: &Path, target: &Path) {
    sync_directory(&source.join("src"), &target.join("src"));
    for file in [
        "tsconfig.json",
        "tsconfig.base.json",
        "tsconfig.app.json",
        "eslint.config.js",
        "package.json",
    ] {
        sync_file(&source.join(file), &target.join(file));
    }
}

fn assert_frontend_checks(source: &Path, target: &Path) {
    let vue_tsc = source.join("node_modules/vue-tsc/bin/vue-tsc.js");
    let typecheck = Command::new("node")
        .arg(vue_tsc)
        .args(["--noEmit", "-p", "tsconfig.app.json"])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 vue-tsc");
    assert_command_succeeded("Order/Notice/Post vue-tsc", &typecheck);

    let eslint = source.join("node_modules/eslint/bin/eslint.js");
    let lint = Command::new("node")
        .arg(eslint)
        .args([
            "src/generated/resources/order",
            "src/generated/resources/notice",
            "src/generated/resources/post",
            "--max-warnings=0",
        ])
        .current_dir(target)
        .output()
        .expect("应运行临时前端 ESLint");
    assert_command_succeeded("Order/Notice/Post ESLint", &lint);
}

fn sync_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("创建目录 {} 失败：{error}", target.display()));
    let source_names = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", source.display()))
        .map(|entry| entry.expect("应读取目录项").file_name())
        .collect::<std::collections::BTreeSet<_>>();
    for entry in fs::read_dir(target)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", target.display()))
    {
        let entry = entry.expect("应读取目标目录项");
        if source_names.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().expect("应读取目标文件类型").is_dir() {
            fs::remove_dir_all(&path)
                .unwrap_or_else(|error| panic!("删除旧目录 {} 失败：{error}", path.display()));
        } else {
            fs::remove_file(&path)
                .unwrap_or_else(|error| panic!("删除旧文件 {} 失败：{error}", path.display()));
        }
    }
    let mut entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("读取目录 {} 失败：{error}", source.display()))
        .collect::<Result<Vec<_>, _>>()
        .expect("应读取目录项");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().expect("应读取文件类型").is_dir() {
            sync_directory(&source_path, &target_path);
        } else {
            sync_file(&source_path, &target_path);
        }
    }
}

fn sync_file(source: &Path, target: &Path) {
    let unchanged = target.is_file()
        && fs::read(source).expect("应读取源文件") == fs::read(target).expect("应读取目标文件");
    if unchanged {
        return;
    }
    fs::copy(source, target).unwrap_or_else(|error| {
        panic!(
            "复制 {} 到 {} 失败：{error}",
            source.display(),
            target.display()
        )
    });
}

fn write_order_fake_transaction_test(backend: &Path) {
    let tests = backend.join("crates/ryframe-application/tests");
    fs::create_dir_all(&tests).expect("应创建临时 application tests");
    fs::write(
        tests.join("generated_order_fake.rs"),
        r#"use chrono::Utc;
use ryframe_application::TransactionAuditMode;
use ryframe_application::generated::order::{
    OrderFailure, OrderFakePersistence, OrderPersistencePort, OrderRecord,
};

fn record(id: i64) -> OrderRecord {
    OrderRecord {
        tenant_id: "tenant-a".into(),
        id: id + 100,
        name: format!("order-{id}"),
        status: 1,
        created_at: Utc::now(),
        updated_at: None,
        del_flag: 0,
    }
}

#[tokio::test]
async fn transaction_view_only_becomes_visible_after_successful_commit() {
    let fake = OrderFakePersistence::default();

    let rolled_back = fake.begin("tenant-a").await.unwrap();
    rolled_back.insert(record(1)).await.unwrap();
    rolled_back.rollback().await.unwrap();
    assert!(fake.find_by_id("tenant-a", 1).await.unwrap().is_none());

    let commit_failed = fake.begin("tenant-a").await.unwrap();
    commit_failed.insert(record(2)).await.unwrap();
    fake.fail_next(OrderFailure::Commit);
    assert!(
        commit_failed
            .commit(TransactionAuditMode::Skip)
            .await
            .is_err()
    );
    assert!(fake.find_by_id("tenant-a", 2).await.unwrap().is_none());

    let first = fake.begin("tenant-a").await.unwrap();
    let second = fake.begin("tenant-a").await.unwrap();
    first.insert(record(3)).await.unwrap();
    second.insert(record(4)).await.unwrap();
    first.commit(TransactionAuditMode::Skip).await.unwrap();
    second.commit(TransactionAuditMode::Skip).await.unwrap();
    assert!(fake.find_by_id("tenant-a", 3).await.unwrap().is_some());
    assert!(fake.find_by_id("tenant-a", 4).await.unwrap().is_some());
}
"#,
    )
    .expect("应写入临时 Fake 事务测试");
}

fn assert_command_succeeded(label: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{label} 失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
